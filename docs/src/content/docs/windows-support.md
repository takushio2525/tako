---
title: Windows 対応状況
description: macOS 先行で開発している機能のうち、Windows でどこまで使えるかの一覧。
---

:::caution[このページは自動生成です]
内容は tako 本体の対応マトリクス（`crates/tako-core/src/platform/support.rs`）から
`scripts/gen-windows-support-docs.mjs` で生成しています。手で編集しないでください。
:::

tako は **macOS で先行開発し、安定した差分を Windows へ反映する**進め方をとっています。
Windows 版は現在**テスター向けプレビュー**で、まだ macOS 版と同じではありません。
このページは「いま Windows で何が使えるか」を機能ごとに示します。

手元の環境での状態は `tako platform` でいつでも確認できます。

```bash
tako platform                  # この環境の対応状況
tako platform --status pending # 未対応のものだけ
```

## 全体

| 状態 | 件数 | 意味 |
| --- | ---: | --- |
| 対応済み | 93 | macOS 版と同じように使えます |
| 一部対応 | 9 | 使えますが機能が落ちます（理由は各表に記載） |
| 未対応 | 23 | まだ実装されていません（追跡 Issue つき） |
| 対象外 | 1 | Windows には概念自体が存在しません |
| **合計** | **126** | |

## ターミナルの基本

シェルの起動・入出力・スクロール・コピー & ペースト。（このカテゴリ全体としては **対応済み**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_send_input` | 対応済み | — |
| `tako_read_pane` | 対応済み | — |
| `tako_scroll_pane` | 対応済み | — |
| `tako_list_panes` | 対応済み | — |
| `tako_logs` | 対応済み | — |
| `tako_limit_service` | 対応済み | — |
| `tako_theme` | 対応済み | — |
| `tako_lang` | 対応済み | — |
| `tako_settings` | 対応済み | — |
| `tako_check_health` | 対応済み | — |
| `tako_telemetry` | 対応済み | — |
| `tako_platform` | 対応済み | — |

## タブ・ペイン・ウィンドウ

分割 / 移動 / リサイズ、たまり場（バックグラウンド退避）、複数ウィンドウ、メニューバー。（このカテゴリ全体としては **一部対応**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_split_pane` | 対応済み | — |
| `tako_close_pane` | 対応済み | — |
| `tako_focus_pane` | 対応済み | — |
| `tako_resize_pane` | 対応済み | — |
| `tako_equalize_layout` | 対応済み | — |
| `tako_create_tab` | 対応済み | — |
| `tako_select_tab` | 対応済み | — |
| `tako_rename_tab` | 対応済み | — |
| `tako_reorder_tab` | 対応済み | — |
| `tako_move_pane_to_tab` | 対応済み | — |
| `tako_collapse_tab` | 対応済み | — |
| `tako_confirm_close` | 対応済み | — |
| `tako_set_title` | 対応済み | — |
| `tako_auto_rename` | 一部対応 | AI による命名は claude CLI の解決が Windows で効かないため働かず、ヒューリスティック命名にとどまる |
| `tako_window` | 対応済み | — |
| `tako_menu` | 対応済み（macOS でも一部対応） | — |
| `tako_panel` | 対応済み | — |
| `tako_background_pane` | 対応済み | — |
| `tako_foreground_pane` | 対応済み | — |
| `tako_background_list` | 対応済み | — |
| `tako_background_kill` | 対応済み | — |
| `tako_open_dir` | 対応済み | — |
| `tako_recent` | 対応済み | — |
| `tako_ssh_hosts` | 対応済み | — |
| `tako_open_remote` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |

## オーケストレーション（tako master）

worker の spawn・監視・報告・タスク管理。（このカテゴリ全体としては **一部対応**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_orchestrator_spawn` | 対応済み | — |
| `tako_orchestrator_self` | 対応済み | — |
| `tako_orchestrator_worker_status` | 対応済み | — |
| `tako_orchestrator_workers` | 対応済み | — |
| `tako_orchestrator_report` | 未対応 | ペイン外からの採取（scrollback）に到達手段が要る。psmux 等の器を導入していない Windows では取得できない <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |
| `tako_orchestrator_respond` | 対応済み | — |
| `tako_orchestrator_handoff` | 対応済み | — |
| `tako_orchestrator_run` | 対応済み | — |
| `tako_orchestrator_run_status` | 対応済み | — |
| `tako_orchestrator_run_result` | 対応済み | — |
| `tako_orchestrator_supervisor` | 対応済み | — |
| `tako_orchestrator_ledger` | 対応済み | — |
| `tako_orchestrator_projects` | 対応済み | — |
| `tako_orchestrator_profiles` | 対応済み | — |
| `tako_orchestrator_accounts` | 対応済み | — |
| `tako_orchestrator_layout` | 対応済み | — |
| `tako_task_checkpoint` | 対応済み | — |
| `tako_task_list` | 対応済み | — |
| `tako_task_resume` | 対応済み | — |
| `tako_task_gate` | 対応済み | — |
| `tako_task_gate_check` | 対応済み | — |
| `tako_task_gate_show` | 対応済み | — |

## セッション永続化

tako を再起動したときにタブ・ペインと実行中プロセスをどこまで戻せるか。（このカテゴリ全体としては **一部対応**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_persist` | 一部対応 | psmux（tmux 互換の永続化バックエンド）を導入すると実行中プロセスと画面ごと復元する。未導入ならタブ・ペイン構成と cwd のみ復元し、実行中プロセスは tako の終了時に停止する |
| `tako_sessions` | 未対応 | tmux バックエンドに依存。Windows の永続化戦略の決定が前提 <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |
| `tako_tmux_list` | 未対応 | tmux サーバーそのものを操作する機能。Windows に tmux は無い <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |
| `tako_tmux_open` | 未対応 | tmux サーバーそのものを操作する機能。Windows に tmux は無い <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |
| `tako_tmux_kill` | 未対応 | tmux サーバーそのものを操作する機能。Windows に tmux は無い <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |
| `tako_tmux_cleanup` | 未対応 | tmux サーバーそのものを操作する機能。Windows に tmux は無い <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |
| `tako_tmux_resize` | 未対応 | tmux サーバーそのものを操作する機能。Windows に tmux は無い <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |
| `tako_tmux_select_window` | 未対応 | tmux サーバーそのものを操作する機能。Windows に tmux は無い <br />追跡: [#519](https://github.com/takushio2525/tako/issues/519) |

## git 連携

右パネルの git タブ（履歴 / diff / ステージング / ブランチ操作 / コンフリクト解消）。（このカテゴリ全体としては **対応済み**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_git_log` | 対応済み | — |
| `tako_git_diff` | 対応済み | — |
| `tako_git_show` | 対応済み | — |
| `tako_git_stage` | 対応済み | — |
| `tako_git_unstage` | 対応済み | — |
| `tako_git_commit` | 対応済み | — |
| `tako_git_push` | 対応済み | — |
| `tako_git_pull` | 対応済み | — |
| `tako_git_branch_create` | 対応済み | — |
| `tako_git_checkout` | 対応済み | — |
| `tako_git_merge` | 対応済み | — |
| `tako_git_merge_abort` | 対応済み | — |
| `tako_git_conflicts` | 対応済み | — |
| `tako_git_resolve_agent` | 対応済み | — |

## ファイルプレビュー・Web ビュー

コード / Markdown / 画像 / PDF / 動画のプレビューと、ネイティブ Web ビューペイン。（このカテゴリ全体としては **一部対応**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_open_file` | 一部対応 | コード・Markdown・画像は表示できる。PDF と動画は macOS 実装のため表示できない |
| `tako_preview_view` | 一部対応 | 画像のズーム・パンは動く。PDF は macOS 実装のため開けず操作対象にならない |
| `tako_preview_outline` | 対応済み | — |
| `tako_preview_reload` | 対応済み | — |
| `tako_preview_cache` | 対応済み | — |
| `tako_preview_changelog` | 対応済み | — |
| `tako_preview_search` | 対応済み | — |
| `tako_preview_edit` | 対応済み | — |
| `tako_preview_apply` | 対応済み | — |
| `tako_preview_replace` | 対応済み | — |
| `tako_preview_save` | 対応済み | — |
| `tako_preview_undo` | 対応済み | — |
| `tako_preview_redo` | 対応済み | — |
| `tako_preview_autosave` | 対応済み | — |
| `tako_preview_link_list` | 未対応 | PDF プレビュー専用の操作。PDF の描画が macOS（PDFKit）実装のため Windows では開けない <br />追跡: [#521](https://github.com/takushio2525/tako/issues/521) |
| `tako_preview_follow_link` | 未対応 | PDF プレビュー専用の操作。PDF の描画が macOS（PDFKit）実装のため Windows では開けない <br />追跡: [#521](https://github.com/takushio2525/tako/issues/521) |
| `tako_pin_preview` | 対応済み | — |
| `tako_video_playback` | 未対応 | 動画プレビューが macOS（AVFoundation）実装のため Windows では再生できない <br />追跡: [#521](https://github.com/takushio2525/tako/issues/521) |
| `tako_video_seek` | 未対応 | 動画プレビューが macOS（AVFoundation）実装のため Windows では再生できない <br />追跡: [#521](https://github.com/takushio2525/tako/issues/521) |
| `tako_video_volume` | 未対応 | 動画プレビューが macOS（AVFoundation）実装のため Windows では再生できない <br />追跡: [#521](https://github.com/takushio2525/tako/issues/521) |
| `tako_web` | 対応済み | — |

## コード実行（Code Runner）

プレビューの再生ボタン・拡張子既定コマンド・対話コマンドの委譲。（このカテゴリ全体としては **一部対応**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_run` | 一部対応 | 実行ペインは PowerShell で動く。ただし PowerShell 7 が無く Windows PowerShell 5.1 だけの環境では、`&&` / `\|\|` でつないだコマンド（C / C++ / Rust の拡張子既定を含む）が構文エラーになる。PowerShell 7 を入れると解消する |
| `tako_run_resolve` | 対応済み | — |
| `tako_run_defaults` | 対応済み | — |
| `tako_run_interactive` | 一部対応 | 実行ペインは PowerShell で動く。ただし PowerShell 7 が無く Windows PowerShell 5.1 だけの環境では、`&&` / `\|\|` でつないだコマンド（C / C++ / Rust の拡張子既定を含む）が構文エラーになる。PowerShell 7 を入れると解消する |
| `tako_run_interactive_status` | 一部対応 | 実行ペインは PowerShell で動く。ただし PowerShell 7 が無く Windows PowerShell 5.1 だけの環境では、`&&` / `\|\|` でつないだコマンド（C / C++ / Rust の拡張子既定を含む）が構文エラーになる。PowerShell 7 を入れると解消する |

## セットアップ・OS 連携

初回セットアップ、MCP 登録、ファイル操作、ポート検知、スリープ防止。（このカテゴリ全体としては **一部対応**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_setup` | 一部対応 | 環境チェック・設定の生成・MCP 登録・winget での導入案内は動く。シェル統合（PowerShell）は Windows 未対応のため、状態の表示だけで設定はできない |
| `tako_setup_changes` | 対応済み | — |
| `tako_setup_mcp` | 対応済み | — |
| `tako_agents_sync_rules` | 対応済み | — |
| `tako_stale_binary` | 対応済み | — |
| `tako_tree_folder` | 対応済み | — |
| `tako_file_op` | 対応済み | — |
| `tako_port_detect` | 対応済み | — |
| `tako_sleep_guard` | 一部対応 | アイドルスリープの防止は動く。蓋を閉じたまま走らせ続ける設定と本体温度の監視は macOS 固有の仕組みのため Windows には無い（蓋を閉じたときの動作は電源プランに従う） |
| `tako_fda` | 対象外 | Windows に macOS の TCC（フルディスクアクセス）に相当する仕組みが無い |

## リモートアクセス・自動更新

スマホからの接続（tako remote）とアプリ内アップデート。（このカテゴリ全体としては **未対応**）

| 機能 | Windows | 補足 |
| --- | --- | --- |
| `tako_remote_start` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_remote_stop` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_remote_status` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_remote_setup` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_remote_devices` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_remote_agents` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_remote_messages` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_remote_scrollback` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |
| `tako_update` | 未対応 | remote トランスポートと Windows 配布系統が前提 <br />追跡: [#528](https://github.com/takushio2525/tako/issues/528) |

## この表の読み方

- 機能名は **MCP ツール名**です。同じ操作が CLI（`tako …`）と GUI からもできます
  （tako の開発原則「UI でできることはすべて AI からもできる」）。
  対応する CLI は [CLI リファレンス](/guides/cli-reference/)、
  ツールの詳細は [MCP ツール一覧](/guides/mcp-tools/)を参照してください
- **未対応の機能も一覧から消していません**。消すと AI エージェントが
  「そんな機能は無い」と誤認して回避行動も取れなくなるためです。
  未対応の操作を呼ぶと、理由と追跡 Issue を含むエラーが返ります
- 縮退の理由は tako 本体の 1 箇所で定義していて、このページ・`tako platform`・
  エージェントの system prompt がすべて同じ文言を使います

