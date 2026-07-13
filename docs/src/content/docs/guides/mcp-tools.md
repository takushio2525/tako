---
title: MCP ツール一覧
description: tako が AI エージェントに公開する 50+ の MCP ツールの全リスト
---

tako は 50 以上の MCP ツールを公開しています。AI エージェント（Claude Code 等）はこれらのツールを使って tako を操作します。CLI コマンドと 1:1 で対応しているため、各ツールの詳しい動きは [CLI リファレンス](/guides/cli-reference/)の対応コマンドを参照してください。

:::tip
MCP ツールの登録は `tako setup`（または `tako setup-mcp`）で一度だけ行えば、以降はどのプロジェクトでも自動的に使えます。
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
| `tako_scroll_pane` | ペインのスクロール位置を操作 |

## テキスト操作

| ツール名 | 説明 |
|---|---|
| `tako_send_input` | ペインにテキストを送信（claude TUI へは送達確認付き） |
| `tako_read_pane` | ペインの画面内容を取得 |
| `tako_set_title` | ペインのタイトル・役割ラベルを設定 |

## タブ操作

| ツール名 | 説明 |
|---|---|
| `tako_create_tab` | 新しいタブを作成 |
| `tako_select_tab` | タブを切り替え |
| `tako_rename_tab` | タブ名を変更 |
| `tako_move_pane_to_tab` | ペインを別タブに移動 |
| `tako_collapse_tab` | サイドバーのタブ枠折りたたみを切替 |

## ファイル・プレビュー

| ツール名 | 説明 |
|---|---|
| `tako_open_file` | ファイルをプレビューペインで表示（コード / Markdown / 画像 / PDF / 動画） |
| `tako_file_op` | ファイル操作（パスコピー / Finder 表示 / cd / リネーム / 作成 / ゴミ箱） |
| `tako_video_playback` | 動画プレビューの再生 / 一時停止 |
| `tako_video_seek` | 動画プレビューのシーク |
| `tako_web` | Web ビューペインの操作（open / list / show / hide / close / navigate / eval / eval_result / read）。ネイティブ WKWebView でユーザーが直接操作でき、hide でページを生かしたまま dock へ退避できる |
| `tako_pin_preview` | プレビューのピン留め（フローティングウィンドウ化） |

## バックグラウンド退避（たまり場）

| ツール名 | 説明 |
|---|---|
| `tako_background_pane` | ペインをバックグラウンドへ退避（プロセスは維持） |
| `tako_foreground_pane` | 退避中ペインを画面に復帰 |
| `tako_background_list` | 退避中ペインの一覧 |
| `tako_background_kill` | 退避中ペインを完全に破棄 |

## tmux 管理

| ツール名 | 説明 |
|---|---|
| `tako_tmux_list` | tmux セッション一覧（tako ペインとの対応付き） |
| `tako_tmux_kill` | tmux セッション / window を終了 |
| `tako_tmux_open` | 外部 tmux セッションをタブに取り込み |
| `tako_tmux_select_window` | tmux window を切り替え |
| `tako_tmux_resize` | tmux window のリサイズ |
| `tako_tmux_cleanup` | 取り残されたセッションの一括掃除 |

## git 連携

| ツール名 | 説明 |
|---|---|
| `tako_git_log` | コミット履歴・ブランチ・変更状態の取得 |
| `tako_git_diff` | git diff の取得（unstaged / staged / コミット指定） |

## パネル・設定

| ツール名 | 説明 |
|---|---|
| `tako_panel` | サイドバーの表示 / 非表示・ビュー切替・ファイルツリー |
| `tako_persist` | tmux バックエンド永続化の ON/OFF・診断情報 |
| `tako_port_detect` | ポート検知の ON/OFF |
| `tako_auto_rename` | AI 自動リネームの ON/OFF |
| `tako_setup_mcp` | MCP 接続設定の追加 |
| `tako_setup_changes` | setup のアップデート追従状況（未適用の setup 関連変更）の照会 |
| `tako_update` | アプリ更新の診断・チェック・実行 |
| `tako_check_health` | 接続状態の確認 |

## リモートアクセス

| ツール名 | 説明 |
|---|---|
| `tako_remote_start` | リモートアクセスサーバーの起動 |
| `tako_remote_stop` | 同・停止 |
| `tako_remote_status` | 同・状態確認（トークンは既定でマスク。生値は `show_token=true`） |
| `tako_remote_agents` | 動作中エージェントの一覧 |
| `tako_remote_messages` | エージェントの会話ログ取得 |
| `tako_remote_scrollback` | ペインのスクロールバック履歴取得 |

## オーケストレーター

| ツール名 | 説明 |
|---|---|
| `tako_orchestrator_spawn` | 子 worker を起動（`tab` / `pane` で出力先を指定） |
| `tako_orchestrator_run` | spawn + 完了待ち + 出力回収 + 片付けをワンショット実行 |
| `tako_orchestrator_worker_status` | worker の状態確認 |
| `tako_orchestrator_projects` | プロジェクト管理（一覧 / 追加 / 削除） |
| `tako_orchestrator_profiles` | プロファイル管理（一覧 / 表示 / 設定） |

## ペインの自動特定

MCP ツールは呼び出し元のペイン ID を自動で認識します。stdio ブリッジの場合は環境変数 `TAKO_PANE_ID`、HTTP の場合は `X-Tako-Pane` ヘッダーから取得します。

ペイン ID を省略した場合のデフォルト対象は呼び出し元ペインになるため、AI は「自分のペインの隣にペインを作る」操作を自然に行えます。タブをまたぐ操作には明示的な ID 指定が必要です。
