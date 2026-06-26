---
title: MCP ツール一覧
description: tako が公開する 40+ の MCP ツールの全リスト
---

tako は 40 以上の MCP ツールを公開しています。AI エージェント（Claude Code 等）はこれらのツールを使って tako を操作します。

:::tip
MCP ツールの登録は `tako setup-mcp` で一度だけ行えば、以降はどのプロジェクトでも自動的に使えます。
:::

## レイアウト操作

| ツール名 | 説明 |
|---|---|
| `tako_split_pane` | ペインを分割（方向・比率・実行コマンド指定可） |
| `tako_close_pane` | ペインを閉じる（自分自身の削除も可） |
| `tako_focus_pane` | 指定ペインにフォーカスを移動 |
| `tako_resize_pane` | ペインのサイズを変更 |
| `tako_equalize_layout` | タブ内の全ペインを均等化 |
| `tako_list_panes` | タブ・ペインの全構成を JSON で取得 |
| `tako_foreground_pane` | ペインを前面に表示 |
| `tako_background_pane` | ペインをバックグラウンドに |
| `tako_scroll_pane` | ペインのスクロール位置を操作 |

## テキスト操作

| ツール名 | 説明 |
|---|---|
| `tako_send_input` | ペインにテキスト/キー入力を送信 |
| `tako_read_pane` | ペインの画面内容を取得 |
| `tako_set_title` | ペインのタイトルを設定 |

## タブ操作

| ツール名 | 説明 |
|---|---|
| `tako_create_tab` | 新しいタブを作成 |
| `tako_select_tab` | タブを切り替え |
| `tako_rename_tab` | タブ名を変更 |
| `tako_move_pane_to_tab` | ペインを別タブに移動 |
| `tako_collapse_tab` | タブの折りたたみを切替 |

## ファイル・プレビュー

| ツール名 | 説明 |
|---|---|
| `tako_open_file` | ファイルをプレビューペインで表示 |
| `tako_file_op` | ファイル操作（copy / move / rename / delete / mkdir） |

## たまり場

| ツール名 | 説明 |
|---|---|
| `tako_shelve_pane` | ペインをたまり場へ退避 |
| `tako_unshelve_pane` | 退避ペインをタブに復帰 |
| `tako_shelved_list` | 退避中ペインの一覧 |
| `tako_shelved_kill` | 退避ペインを完全に破棄 |

## tmux 管理

| ツール名 | 説明 |
|---|---|
| `tako_tmux_list` | tmux セッション一覧 |
| `tako_tmux_kill` | tmux セッションを終了 |
| `tako_tmux_open` | 外部 tmux セッションをタブに取り込み |
| `tako_tmux_select_window` | tmux window を選択 |
| `tako_tmux_cleanup` | orphan セッションの一括掃除 |

## パネル表示

| ツール名 | 説明 |
|---|---|
| `tako_panel` | 右サイドバーの表示/非表示・ビュー切替 |
| `tako_pin_preview` | プレビューのピン留め（フローティングウィンドウ化） |

## git 連携

| ツール名 | 説明 |
|---|---|
| `tako_git_log` | git コミットログの取得 |
| `tako_git_diff` | git diff の取得 |

## 設定

| ツール名 | 説明 |
|---|---|
| `tako_persist` | tmux バックエンドの ON/OFF |
| `tako_port_detect` | ポート検知の ON/OFF |
| `tako_auto_rename` | AI 自動リネームの ON/OFF |

## オーケストレーター

| ツール名 | 説明 |
|---|---|
| `tako_orchestrator_spawn` | 子 worker を起動 |
| `tako_orchestrator_worker_status` | worker の状態確認 |
| `tako_orchestrator_projects` | プロジェクト管理（一覧/追加/削除） |

## その他

| ツール名 | 説明 |
|---|---|
| `tako_check_health` | 接続状態の確認 |
| `tako_background_list` | バックグラウンドプロセス一覧 |
| `tako_background_kill` | バックグラウンドプロセスの終了 |

## ペインの自動特定

MCP ツールは呼び出し元のペイン ID を自動で認識します。stdio ブリッジの場合は環境変数 `TAKO_PANE_ID`、HTTP の場合は `X-Tako-Pane` ヘッダーから取得します。

ペイン ID を省略した場合のデフォルト対象は呼び出し元ペインになるため、AI は「自分のペインの隣にペインを作る」操作を自然に行えます。タブをまたぐ操作には明示的な ID 指定が必要です。
