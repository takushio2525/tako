---
title: MCP ツール一覧
description: tako が AI エージェントに公開する 139 個の MCP ツールの全リスト
---

tako は **139 個の MCP ツール**を AI エージェント（Claude Code / Codex 等）に公開しています。ほぼすべてが `tako` CLI のコマンドと 1:1 で対応しているため、細かい引数や挙動は [CLI リファレンス](/guides/cli-reference/)の対応コマンドも合わせて参照してください。

:::tip[登録は一度きり]
MCP ツールの登録は `tako setup`（または `tako setup-mcp`）で一度行えば、以降はどのプロジェクトでも自動的に使えます。codex を master にする場合は `tako master` の起動時にだけ設定が注入されるため、グローバル設定の変更すら不要です。
:::

:::note[この一覧の作り方]
このページの一覧は tako 本体のツール定義（`tako-control` の `mcp::tools()`）から機械的に抽出したものです。数と名前は実装のスナップショット（`crates/tako-app/testdata/mcp_tools_snapshot.txt`）と一致しています。
:::

## 画面とレイアウト

| ツール名 | 説明 |
|---|---|
| `tako_list_panes` | タブ・ペインのツリー構造・ジオメトリ・状態を JSON で取得。**操作前にまず呼ぶ** |
| `tako_split_pane` | ペインを分割して新ペインを作る（方向・比率・実行コマンド指定可） |
| `tako_close_pane` | ペインを閉じる |
| `tako_focus_pane` | 指定ペインへフォーカスを移す |
| `tako_resize_pane` | ペインの取り分（サイズ比率）を変える |
| `tako_equalize_layout` | タブ内の全ペインを均等化する |
| `tako_scroll_pane` | スクロールバック表示を動かす |
| `tako_move_pane_to_tab` | ペインを別のタブへ移動する |
| `tako_window` | 複数ウィンドウの操作（一覧 / 新規 / 閉じる / タブ移動 / フォーカス） |

## テキストの読み書き

| ツール名 | 説明 |
|---|---|
| `tako_send_input` | ペインへテキストを送信する（全画面 TUI へは送達確認付きで配送） |
| `tako_read_pane` | ペインの画面内容をテキストで取得する |
| `tako_set_title` | ペインの表示タイトルと役割ラベル（role）を設定する |
| `tako_show_command` | コピー / 実行ボタンつきのコマンド提案カードを出す（会話に直書きすると折り返しでコピーが壊れるため） |

## タブ

| ツール名 | 説明 |
|---|---|
| `tako_create_tab` | 新しいタブ（作業グループ）を作る |
| `tako_select_tab` | 表示するタブを切り替える |
| `tako_rename_tab` | タブ名を変更する |
| `tako_reorder_tab` | タブの並び順を変更する |
| `tako_pin_tab_title` | 今のタブ名を固定する（以後 AI 自動リネームの対象外にする） |
| `tako_collapse_tab` | サイドバーのタブ枠を折りたたむ / 展開する |

## ファイルとプレビュー

| ツール名 | 説明 |
|---|---|
| `tako_open_file` | ファイルをプレビューペインで開く（コード / Markdown / 画像 / PDF / 動画） |
| `tako_file_op` | ファイル操作（パスコピー / Finder 表示 / cd / リネーム / 作成 / ゴミ箱 / 既定アプリで開く） |
| `tako_tree_folder` | ファイルツリーへフォルダを追加・削除・一覧する / git ステータスを取得する（`action: "git-status"`） |
| `tako_preview_view` | PDF・画像のズーム / ページ / パン操作 |
| `tako_preview_outline` | Markdown 見出し・PDF 目次のアウトライン表示とジャンプ |
| `tako_preview_link_list` | PDF 内のリンク一覧を取得する |
| `tako_preview_follow_link` | PDF 内のリンクをたどる |
| `tako_preview_reload` | プレビューのライブリロードの ON/OFF・状態確認 |
| `tako_preview_copy_code` | Markdown プレビューのコードブロックを装飾なしでコピーする |
| `tako_preview_cache` | デコード済みプレビュー画像キャッシュの上限と使用状況 |
| `tako_preview_changelog` | プレビューのチェンジログ（git 履歴）ビュー切替・diff 展開 |
| `tako_pin_preview` | プレビューをピン留めしてフローティングウィンドウ化する |

### プレビューの編集

| ツール名 | 説明 |
|---|---|
| `tako_preview_edit` | コードプレビューの編集モードを開始・終了する |
| `tako_preview_apply` | 編集バッファの全文を差し替える |
| `tako_preview_save` | 未保存の編集をファイルへ保存する |
| `tako_preview_undo` / `tako_preview_redo` | 編集の undo / redo |
| `tako_preview_search` / `tako_preview_replace` | テキストの検索 / 置換 |
| `tako_preview_autosave` | 編集の自動保存設定 |

### 動画

| ツール名 | 説明 |
|---|---|
| `tako_video_playback` | 再生 / 一時停止 / 音量の切替 |
| `tako_video_seek` | 再生位置のシーク |
| `tako_video_volume` | 音量の設定 |

## git

| ツール名 | 説明 |
|---|---|
| `tako_git_log` | コミット履歴・ブランチ一覧・変更状態を取得する |
| `tako_git_diff` | diff をファイル / ハンク / 行単位で取得する |
| `tako_git_show` | コミット詳細（メタ情報・変更ファイル一覧）を取得する |
| `tako_git_stage` / `tako_git_unstage` | ファイルのステージ / アンステージ |
| `tako_git_commit` | コミットする |
| `tako_git_push` / `tako_git_pull` | push / pull |
| `tako_git_checkout` | ブランチを切り替える（既定は予行演習。実行は明示指定） |
| `tako_git_branch_create` | 新規ブランチを作成する |
| `tako_git_merge` | マージする（既定は予行演習。コンフリクトを事前予測） |
| `tako_git_merge_abort` | 進行中の merge / rebase / cherry-pick / revert を中止する |
| `tako_git_conflicts` | コンフリクト状態を取得する |
| `tako_git_resolve_agent` | コンフリクト解消エージェントをペインで起動する |

## バックグラウンド退避（たまり場）

| ツール名 | 説明 |
|---|---|
| `tako_background_pane` | ペイン / タブをバックグラウンドへ退避する（プロセスは維持） |
| `tako_foreground_pane` | 退避中ペインを画面へ復帰させる |
| `tako_background_list` | 退避中ペインの一覧 |
| `tako_background_kill` | 退避中ペインを完全に破棄する |

## tmux 管理

| ツール名 | 説明 |
|---|---|
| `tako_tmux_list` | tmux セッション一覧（tako のペインとの対応付き） |
| `tako_tmux_kill` | セッション / window を終了する |
| `tako_tmux_open` | 外部の tmux セッションを現在のタブへ取り込む |
| `tako_tmux_select_window` | バックエンドセッション内の window を切り替える |
| `tako_tmux_resize` | window を指定サイズへリサイズする |
| `tako_tmux_cleanup` | 取り残された orphan セッションを一括で片付ける |
| `tako_persist` | セッション永続化（tmux バックエンド）の ON/OFF・診断情報 |

## オーケストレーター

| ツール名 | 説明 |
|---|---|
| `tako_orchestrator_spawn` | 子 worker をペインに起動してプロンプトを渡す |
| `tako_orchestrator_run` | spawn し `run_id` を返す（非同期ワンショット実行） |
| `tako_orchestrator_run_status` | 非同期 run の進捗を照会する |
| `tako_orchestrator_run_result` | 完了した非同期 run の結果を回収する |
| `tako_orchestrator_worker_status` | worker の状態確認（busy / idle / error / gone / 権限待ち） |
| `tako_orchestrator_workers` | worker レジストリの一覧（ペインが消えても追跡できる） |
| `tako_orchestrator_report` | worker の報告内容を取得する（scrollback + transcript の 2 層） |
| `tako_orchestrator_respond` | worker の権限確認ダイアログに応答する |
| `tako_orchestrator_supervisor` | worker 自動復旧 supervisor の操作 |
| `tako_orchestrator_self` | master / solo が自分の pane・tab・コンテキスト残量・引き継ぎ閾値を取得する |
| `tako_orchestrator_handoff` | master の引き継ぎ（新しい master へバトンを渡す。渡るのは担当プロジェクトの引き継ぎだけ。前任のペインは後任が確認後に閉じる） |
| `tako_orchestrator_handoffs` | 引き継ぎファイルの管理（一覧 / 読み / 書き / 旧形式の自動移行） |
| `tako_orchestrator_projects` | 管理対象プロジェクトの一覧 / 追加 / 削除 |
| `tako_orchestrator_profiles` | プロファイル（モデル・思考量・エージェント CLI・自動ハンドオフ）の管理 |
| `tako_orchestrator_accounts` | アカウントレジストリの管理（worker ごとの使い分け） |
| `tako_orchestrator_layout` | worker spawn 時のレイアウト方針の設定 |
| `tako_orchestrator_ledger` | 委任台帳の操作 |

## タスク管理

| ツール名 | 説明 |
|---|---|
| `tako_task_checkpoint` | タスクチェックポイントの記録・更新 |
| `tako_task_list` | チェックポイントの一覧 |
| `tako_task_resume` | チェックポイントから worker を再開する |
| `tako_task_gate` | 受け入れゲート（完了条件）の定義 |
| `tako_task_gate_check` | 受け入れゲートを実行して結果を記録する |
| `tako_task_gate_show` | 受け入れゲートの状態を表示する |

## コマンド実行（Code Runner）

| ツール名 | 説明 |
|---|---|
| `tako_run` | ファイルを実行する（`tako:run` 宣言または拡張子既定で新ペイン分割） |
| `tako_run_resolve` | ファイルの実行プロファイル候補を解決する |
| `tako_run_defaults` | 拡張子ごとの実行コマンド既定を一覧 / 設定 / 削除 |
| `tako_run_interactive` | ユーザー入力が必要なコマンドを可視ペインへ委譲する |
| `tako_run_interactive_status` | `run_interactive` で起動したペインの完了状態を確認する |

## リモートアクセス

| ツール名 | 説明 |
|---|---|
| `tako_remote_setup` | Tailscale セットアップの状態確認・実行 |
| `tako_remote_start` / `tako_remote_stop` | リモートアクセスサーバーの起動 / 停止 |
| `tako_remote_status` | 状態確認（固定 URL・登録端末数。secret は含まない） |
| `tako_remote_devices` | ペアリング済み端末の一覧 / 失効 |
| `tako_remote_agents` | 動作中のエージェント一覧 |
| `tako_remote_messages` | エージェントの会話ログ取得 |
| `tako_remote_scrollback` | ペインのスクロールバック履歴取得 |

:::note[承認と権限変更は AI からはできません]
機器ペアリングの**承認**と**権限（role）の変更**は Mac 画面の承認ダイアログ限定で、MCP / CLI には API がありません。これは「tako を操作できる端末を増やす」操作を必ず人間の手を経由させるためのセキュリティ境界です（「AI フルコントロール」原則の明示的な例外）。start / stop / status / devices は AI からも操作できます。
:::

## 画面から開く

| ツール名 | 説明 |
|---|---|
| `tako_open_dir` | ディレクトリを新しいタブで開く |
| `tako_open_remote` | SSH ホストに接続する（`target`: `split` = いまのタブに新ペイン（既定）/ `tab` = 新しいタブ / `pane` = 既存ペインをそのまま SSH 化） |
| `tako_ssh_hosts` | `~/.ssh/config` の Host 一覧を返す |
| `tako_recent` | 最近開いたディレクトリ / リポジトリ / SSH ホストの一覧・クリア |
| `tako_web` | ネイティブ Web ビューペインの操作（開く / 退避 / ナビゲート / JS 評価） |

## 表示・設定

| ツール名 | 説明 |
|---|---|
| `tako_ui_mode` | かんたん表示（GUI モード）とターミナル表示の切替・ペイン単位の解除 / 復帰 |
| `tako_chat_copy` | かんたん表示の会話本文・コードブロックをコピーする |
| `tako_theme` | UI テーマの確認・切替・色設定・プリセット・フォント |
| `tako_lang` | UI 表示言語（日本語 / 英語）の確認・切替 |
| `tako_settings` | 設定画面を開く |
| `tako_panel` | 右サイドバーの表示 / ビュー切替 / 幅、ファイルツリーの表示 |
| `tako_welcome` | 初回起動バナーの状態確認・再表示・非表示 |
| `tako_autosuggest` | 入力予測（ゴーストテキスト）の ON/OFF・確定キーの設定 |
| `tako_auto_rename` | タブ・ペイン名の AI 自動リネームの ON/OFF |
| `tako_port_detect` | listen ポート検知と提案チップの ON/OFF |
| `tako_confirm_close` | 閉じる確認ダイアログの ON/OFF |
| `tako_limit_resume` | 利用上限後の自動復帰のペイン単位 ON/OFF・状態確認 |
| `tako_limit_service` | ステータスバーに出す利用制限表示サービスの切替 |
| `tako_sleep_guard` | スリープ防止の状態確認・設定変更 |

## セットアップ・診断・保守

| ツール名 | 説明 |
|---|---|
| `tako_setup` | `tako setup` を非対話で実行する（日本語の希望を回答 JSON にして代行） |
| `tako_setup_changes` | setup のアップデート追従状況を照会する |
| `tako_setup_mcp` | claude / codex / agy へ MCP 接続設定を追加する（`agent` 省略で導入済みの全 CLI） |
| `tako_migrate` | 設定・データファイルの形式の確認と自動マイグレーションの手動発火 |
| `tako_check_health` | tako 環境の健全性を診断する |
| `tako_platform` | プラットフォーム対応マトリクスの参照（使える / 縮退 / 未実装） |
| `tako_update` | アプリ内更新の診断・チェック・実行 |
| `tako_stale_binary` | 稼働中セッションの claude バイナリの鮮度確認と張り直し |
| `tako_fda` | フルディスクアクセスの状態確認と設定画面の起動 |
| `tako_telemetry` | エラーレポート自動送信（テレメトリ）の状態確認・切替 |
| `tako_agents_sync_rules` | エージェント共通ルールを各 CLI の指示ファイルへ同期する |
| `tako_config_share` | AI 系設定（claude のグローバル指示 + tako の宣言的設定）の git ベース共有 |
| `tako_sessions` | セッションカタログの参照と会話の復元 |
| `tako_logs` | ペインの平文ターミナルログの参照・設定 |

## ペインの自動特定

MCP ツールは呼び出し元のペイン ID を自動で認識します。stdio ブリッジの場合は環境変数 `TAKO_PANE_ID`、HTTP の場合は `X-Tako-Pane` ヘッダーから取得します。

ペイン ID を省略したときの既定の対象は呼び出し元ペインになるため、AI は「自分のペインの隣にペインを作る」操作を自然に行えます。タブをまたぐ操作には明示的な ID 指定が必要です。

## 関連ページ

- [内蔵 MCP サーバー](/features/mcp-server/) — 仕組みと設計思想
- [CLI リファレンス](/guides/cli-reference/) — 対応する `tako` コマンド
