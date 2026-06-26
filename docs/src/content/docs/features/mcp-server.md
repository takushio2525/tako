---
title: 内蔵 MCP サーバー
description: tako 最大の差別化点 — 設定ゼロで AI がターミナルを操作
---

tako は **MCP（Model Context Protocol）サーバーを内蔵**しています。AI エージェントが設定ゼロでペインの分割・コマンド実行・ファイル表示を行えるのが、他のターミナルとの最大の違いです。

## 仕組み

1. tako は起動時に MCP サーバー（Streamable HTTP）を内蔵で立ち上げる
2. 各ペインのシェルに環境変数（`TAKO_PANE_ID`、`TAKO_MCP_URL` 等）を注入
3. AI エージェントは環境変数から MCP サーバーを自動発見し、ツールとして利用

初回に `tako setup-mcp` で stdio ブリッジを登録すれば、以降はどのプロジェクトでも設定不要です。

## 40+ の MCP ツール

tako は 40 以上の MCP ツールを公開しています。主なカテゴリ:

### レイアウト操作
- `tako_split_pane` — ペイン分割（方向・比率・コマンド指定）
- `tako_close_pane` — ペイン削除（自己片付け対応）
- `tako_focus_pane` — フォーカス移動
- `tako_resize_pane` — サイズ調整
- `tako_equalize_layout` — レイアウト均等化
- `tako_list_panes` — タブ・ペイン構成の取得（JSON）

### テキスト操作
- `tako_send_input` — ペインへテキスト/キー入力送信
- `tako_read_pane` — ペインの画面内容取得
- `tako_set_title` — タイトル設定

### ファイル・プレビュー
- `tako_open_file` — ファイルをプレビューペインで表示
- `tako_file_op` — ファイル操作（コピー・移動・リネーム・削除等）

### タブ操作
- `tako_create_tab` — 新規タブ作成
- `tako_select_tab` — タブ切替
- `tako_rename_tab` — タブ名変更
- `tako_move_pane_to_tab` — ペインを別タブへ移動

### たまり場
- `tako_shelve_pane` — ペインをバックグラウンドへ退避
- `tako_unshelve_pane` — 退避ペインを復帰
- `tako_shelved_list` — 退避中ペイン一覧

### tmux 管理
- `tako_tmux_list` — tmux セッション一覧
- `tako_tmux_kill` — tmux セッション終了
- `tako_tmux_open` — 外部 tmux セッションをタブに取り込み

### git 連携
- `tako_git_log` — git コミットログ取得
- `tako_git_diff` — git diff 取得

### オーケストレーター
- `tako_orchestrator_spawn` — 子 worker を起動
- `tako_orchestrator_worker_status` — worker 状態確認
- `tako_orchestrator_projects` — プロジェクト管理

## 設計思想: AI フルコントロール

tako の設計原則は「**UI でできることはすべて AI からもできる**」です。新機能を追加するたびに対応する MCP ツールも同時に提供します。

AI エージェントは自分が何をしているかをユーザーに「見せる」ことも重視しています。MCP ツールの説明文には「レビューを求めるときは成果物をプレビューで見せろ」「方針相談は例を作って並べろ」といった行動規範が埋め込まれており、エージェントが自然に画面を活用するよう誘導しています。

## アクセス制御

- MCP サーバーは `localhost` にのみバインド（外部から接続不可）
- Bearer トークン認証（tako 起動ごとに生成）
- Origin ヘッダー検証（不正な Web サイトからの呼び出し防止）
