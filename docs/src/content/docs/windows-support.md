---
title: Windows 対応状況
description: どの機能が Windows で使えるか。対応マトリクスから自動生成しています
---

tako は macOS で先行開発し、安定した差分を Windows へ反映しています。
このページは **tako 本体が持っている対応マトリクスから生成**しているので、
実装とずれません。手元の環境で最新を引くには次を実行してください。

```sh
tako platform                      # この環境の対応状況
tako platform --status pending      # まだ使えないものだけ
```

## 全体

| 状態 | 件数 | 意味 |
| --- | --- | --- |
| 対応 | 112 / 144（78%） | macOS と同じように使えます |
| 一部対応 | 15 | 使えますが機能が落ちます。落ち方は各表の「差分」列 |
| 未実測 | 1 | 実装はあり macOS と同じ経路を通るが、Windows 実機でまだ動かしていないもの |
| 未対応 | 14 | Windows 側の実装が無い、または動かないことが分かっているもの |
| 対象外 | 2 | Windows にその概念が無い、または OS が同等機能を標準で持つ |

### 「未実測」について

tako は **実機で確かめたものだけを「対応」と書きます**。実装がプラットフォーム
共通で動く見込みがあっても、Windows 実機で 1 度も実行していないものは
「未対応 / 未実測」に置いています。過大に申告すると、この宣言を読んで動く
AI エージェント（tako は対応状況を system prompt へ渡します）が
「使えるはず」と信じて失敗し続けるためです。

各表の「根拠」列が判定の裏づけです。

| 根拠 | 意味 |
| --- | --- |
| 実機セルフテスト | Windows 実機の GUI セルフテスト（通しで失敗 0 件）が実際に通した項目 |
| 実機テスト | Windows 実機の `cargo test` で緑のテスト |
| 実機実測 | Windows 実機で操作を実行して結果を記録したもの |
| OS の仕様 | Windows の仕様・設計判断で、実測する対象がそもそも無いもの |
| 未実測 | まだ実機で動かしていないもの |

## ターミナルの基本

対応 30・一部対応 1・対象外 1

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_split_pane` | 対応 | — | 実機セルフテスト: 項目 2 / 18 / 34（cmd+D・tako split・MCP tako_split_pane） |
| `tako_close_pane` | 対応 | — | 実機セルフテスト: 項目 6 / 28 / 40 / 40b（cmd+W・tako close・非フォーカス側 close・10 周の fd 検査） |
| `tako_focus_pane` | 対応 | — | 実機セルフテスト: 項目 4 / 24（方向フォーカス移動・tako focus） |
| `tako_resize_pane` | 対応 | — | 実機セルフテスト: 項目 5 / 5b / 22（キーボード・境界ドラッグ・tako resize --share-y） |
| `tako_equalize_layout` | 対応 | — | 実機セルフテスト: 項目 23（tako equalize） |
| `tako_move_pane_to_tab` | 対応 | — | 実機セルフテスト: 項目 26 / 68c（tako tab move-pane・target + direction） |
| `tako_scroll_pane` | 対応 | — | 実機セルフテスト: 項目 43 / 44 / 44b（ホイールの出し分け・tako scroll・ピクセル単位スクロール） |
| `tako_send_input` | 対応 | — | 実機セルフテスト: 項目 19（tako send）。非 ASCII は #907 で器の注入口へ迂回済み |
| `tako_read_pane` | 対応 | — | 実機セルフテスト: 項目 20 / 33（tako read・MCP tako_read_pane） |
| `tako_list_panes` | 対応 | — | 実機セルフテスト: 項目 17 / 33（tako list・MCP tako_list_panes） |
| `tako_set_title` | 対応 | — | 実機セルフテスト: 項目 21（tako title --role） |
| `tako_create_tab` | 対応 | — | 実機セルフテスト: 項目 7 / 25 / 116（cmd+T・tako tab new・--cwd 指定） |
| `tako_select_tab` | 対応 | — | 実機セルフテスト: 項目 12 / 27（cmd+1・tako tab select） |
| `tako_rename_tab` | 対応 | — | 実機セルフテスト: 項目 50（tako tab rename） |
| `tako_reorder_tab` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako tab reorder 1 --index 1` でタブ順が tab2,tab1,tab3 へ入れ替わり `--index 0` で戻る |
| `tako_collapse_tab` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako collapse --tab 1 on` が collapsed=true を返し `tako list` の collapsed も true、`off` で戻る |
| `tako_pin_tab_title` | 対応 | — | 実機セルフテスト: 項目 51b（自動命名直後の「この名前を固定」） |
| `tako_confirm_close` | 対応 | — | 実機セルフテスト: 項目 73a〜73f（確認ダイアログの表示・Esc・Enter・即 close） |
| `tako_auto_rename` | 一部対応 | AI 命名は動くが、シェル統合が無い（#525）ためタブの素材（cwd / タイトル / 実行状態）が起動後に変化せず、命名はタブごとに 1 回だけになる。claude を導入していない環境ではタブ名が PowerShell の実行ファイルパスになる（#760） | 実機実測: #722 の Windows 11 実測: 隔離 GUI で AI 経路が走りタブ名が AI 由来（同素材のヒューリスティックは PowerShell のパス由来で別物）。セルフテスト項目 51 / 52（適用・手動優先・ON/OFF）も緑。残る縮退は #760 の実測（素材が不変なので 2 回目以降が発火しない） |
| `tako_autosuggest` | 対象外 | Windows の PowerShell は PSReadLine の予測入力を標準搭載しているため、tako 側の注入は要らない | OS の仕様: PowerShell が PSReadLine の予測入力を標準搭載しているので注入する対象が無い（セルフテスト項目 41c / 41c-2 は zsh 不在で自動スキップ） |
| `tako_window` | 対応 | — | 実機セルフテスト: 項目 77（window new → move-tab）+ #872 で 0 枚化の寿命を Windows 向けに実装（項目 79b） |
| `tako_menu` | 対応 | — | 実機セルフテスト: 項目 118（in-window メニューバー #657 の open / invoke / close） |
| `tako_background_pane` | 対応 | — | 実機セルフテスト: 項目 47b（ー ボタンでバックグラウンドへ退避） |
| `tako_background_list` | 対応 | — | 実機セルフテスト: 項目 47c（ドロワーに実画面プレビューが並ぶ） |
| `tako_background_kill` | 対応 | — | 実機実測: #937 の Windows 11 実測: MCP `tako_background_kill` が killed=2 を返し、`tako backgrounded` が 1 件から空になる |
| `tako_foreground_pane` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako background --pane N` で外したペインが `tako foreground N` で由来タブへ戻る（list の panes が 1 → 1,2） |
| `tako_show_command` | 対応 | — | 実機セルフテスト: 項目 91 / 91b（カードとカード帯）+ #875 で新規ペイン実行を実機実測 |
| `tako_run` | 対応 | — | 実機実測: #875 の実機 before/after: 「PTY を起動できなかった」→ 出力 + __TAKO_EXIT=0。終了コード 4 型・引用符・日本語・psmux 経由まで実測 + セルフテスト項目 91(d) の実行検査が ran=true |
| `tako_run_resolve` | 対応 | — | 実機実測: #875 の実機実測で Code Runner の宣言 / 拡張子既定の解決から実行まで通した（3 経路のうちの 1 つ） |
| `tako_run_defaults` | 対応 | — | 実機テスト: 拡張子既定の登録・削除・一覧は設定ファイル I/O だけで、単体が実機で緑 |
| `tako_run_interactive` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako run-interactive --pane <p> <コマンド>` が新ペインで実行し、`--wait` が exit_code=0 / status=exited を返す（ペインが極端に狭いとマーカーが折り返して検出できない = #651。macOS も同様） |
| `tako_run_interactive_status` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako run-interactive-status <pane>` が exit_code=0 / status=exited を返す（狭いペインの折り返しは #651） |

## 表示とプレビュー

対応 21・一部対応 6・未対応 / 未実測 4

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_open_file` | 対応 | — | 実機セルフテスト: 項目 66 / 66b / 68b / 112 / 114 / 116（dispatch・tako open・direction・新しいタブ） |
| `tako_open_dir` | 対応 | — | 実機実測: #970 の Windows 11 実測（同一バイナリの A/B）: `tako open-in dir <path>` で開いたペインの cwd が `C:\Users\…` になり、そのペインで `tako git log --pane N` が branches / commits を返す。`TAKO_970_LEGACY=1` では cwd が `\\?\C:\…` のまま `git リポジトリではない` で止まる |
| `tako_preview_view` | 一部対応 | PDF はページ画像として表示できるが、Windows のレンダラが文字位置を返さないため文字選択・目次・PDF 内リンクは使えない（#693） | 実機セルフテスト: 項目 66b-2 / 70 / 112 / 114（コード・md・画像は緑。PDF はページ画像だけ通り文字座標の検査はスキップ） |
| `tako_preview_outline` | 一部対応 | PDF はページ画像として表示できるが、Windows のレンダラが文字位置を返さないため文字選択・目次・PDF 内リンクは使えない（#693） | 実機セルフテスト: 項目 114（Markdown 目次のジャンプは緑。PDF 目次は text_layer 不在でスキップ） |
| `tako_preview_link_list` | 一部対応 | PDF はページ画像として表示できるが、Windows のレンダラが文字位置を返さないため文字選択・目次・PDF 内リンクは使えない（#693） | 実機セルフテスト: 項目 90 / 114（Markdown リンク索引は緑。PDF 注釈リンクは不可） |
| `tako_preview_follow_link` | 一部対応 | PDF はページ画像として表示できるが、Windows のレンダラが文字位置を返さないため文字選択・目次・PDF 内リンクは使えない（#693） | 実機セルフテスト: 項目 90（Markdown の ⌘+クリックは緑。URL は cmd /C start で開く。PDF 内リンクは不可） |
| `tako_preview_copy_code` | 対応 | — | 実機セルフテスト: 項目 90 / 114（画面外のコードブロックも含めてコピー） |
| `tako_preview_reload` | 対応 | — | 実機セルフテスト: 項目 66c（実 CLI の ON/OFF と OS イベントでの再生成） |
| `tako_preview_cache` | 対応 | — | 実機セルフテスト: 項目 33d / 66c（MCP と CLI から同じ LRU 上限へ反映） |
| `tako_preview_edit` | 対応 | — | 実機セルフテスト: 項目 66d（tako edit で開始 → 適用 → 保存） |
| `tako_preview_apply` | 対応 | — | 実機セルフテスト: 項目 66d（全文適用） |
| `tako_preview_save` | 対応 | — | 実機セルフテスト: 項目 66d（保存と外部変更の拒否） |
| `tako_preview_undo` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako edit undo --pane <p>` が undone=true を返す（redo と対で実測） |
| `tako_preview_redo` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako edit redo --pane <p>` が redone=true を返す（undo と対で実測） |
| `tako_preview_search` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako edit search <語>` が index=2 / total=2 を返し、`--direction next/prev` で index が 1 ↔ 2 と動く |
| `tako_preview_replace` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako edit replace <前> <後>` と `--all` が replaced を返し、`tako edit save` 後の実ファイルが置換後の内容になる |
| `tako_preview_autosave` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako edit autosave true/false` が状態を往復する。有効化後の CLI / MCP 編集で自動保存が発火しないのは実装が共通なので macOS でも同じ（タイマーを始めるのが GUI 入力経路だけ。#973） |
| `tako_preview_changelog` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako preview-changelog on --pane <p>` が changelog=true / commits=2 を返し `off` で戻る |
| `tako_pin_preview` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako pin --pane N on` が pinned へ矩形つきで載り、`off` で消える |
| `tako_video_playback` | 未対応 / 未実測 | 動画デコーダの Windows 実装が無く、動画ファイルを開くとエラーになる | 実機実測: video_player.rs の非 macOS 実装が Err("動画再生は macOS でのみ対応") を返すスタブ |
| `tako_video_seek` | 未対応 / 未実測 | 動画デコーダの Windows 実装が無く、動画ファイルを開くとエラーになる | 実機実測: video_player.rs の非 macOS 実装が Err を返すスタブ |
| `tako_video_volume` | 未対応 / 未実測 | 動画デコーダの Windows 実装が無く、動画ファイルを開くとエラーになる | 実機実測: video_player.rs の非 macOS 実装が Err を返すスタブ |
| `tako_web` | 未対応 / 未実測 | Web ビューは WebView2 側の巻き戻せない panic でアプリごと落ちるため開けない | 実機実測: セルフテスト項目 71 は WebView2 の非巻き戻し panic（wry/src/webview2/mod.rs:910）でアプリごと落ちるためスキップ |
| `tako_theme` | 対応 | — | 実機セルフテスト: 項目 33b（MCP tako_theme: light 適用 → GUI 反映 → toggle） |
| `tako_lang` | 対応 | — | 実機セルフテスト: 項目 33c（MCP tako_lang: en 適用 → i18n 反映 → system 復帰） |
| `tako_ui_mode` | 一部対応 | 表示モードの切替とチャット表示は動くが、スターターカードのボタンからのコマンド投入が LF + POSIX クォート決め打ちなので Windows では実行されない（#899。PR #931 が実機検証待ち） | 実機セルフテスト: 項目 93 / 94 / 97 / 100 / 114 / 115（G1 スターター〜チャット表示と仮想化は緑）。スターターのボタンの投入経路は main が LF + POSIX クォート決め打ちのまま（#899） |
| `tako_chat_copy` | 対応 | — | 実機セルフテスト: 項目 98 / 115（チャット本文の選択・コピー・索引） |
| `tako_panel` | 対応 | — | 実機セルフテスト: 項目 49 / 56 / 64 / 64b（fleet ビュー・タブ枠・tako panel roundtrip・ファイルツリー経路） |
| `tako_tree_folder` | 対応 | — | 実機セルフテスト: 項目 67 / 85（タブ = ワークスペース・TreeFolder 経由の git セクション） |
| `tako_welcome` | 一部対応 | バナーの表示と案内コマンドの取得は動くが、ボタンからのコマンド投入が LF + POSIX クォート決め打ちなので Windows では実行されない（#899。PR #931 が実機検証待ち） | 実機セルフテスト: 項目 88（初回起動バナーと案内コマンドの取得は緑）。ボタンの投入経路は main が LF + POSIX クォート決め打ちのまま（#899） |
| `tako_recent` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako recent list` が `open-in dir` で開いたディレクトリを返す |

## AI 連携（オーケストレーション）

対応 25・一部対応 2

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_orchestrator_spawn` | 対応 | — | 実機セルフテスト: 項目 72 / 117（配置エンジンとプロファイル適用）+ #867 で実機の claude 起動と env 到達を PEB で確認 |
| `tako_orchestrator_self` | 対応 | — | 実機セルフテスト: 項目 102（後任 master の self がプロファイルと handoff_path を引き継ぐ） |
| `tako_orchestrator_handoff` | 対応 | — | 実機セルフテスト: 項目 101 / 102 / 102b / 102c / 122（自動通知・後任起動・新旧書式・管轄解決） |
| `tako_orchestrator_handoffs` | 対応 | — | 実機実測: #915 で実機 13/13（移行・冪等・list / show / write・日本語本文・円記号区切りのパス）+ セルフテスト項目 122 |
| `tako_orchestrator_profiles` | 対応 | — | 実機セルフテスト: 項目 96 / 99 / 117（設定画面のフォーム・スターターの ▾・limit_resume の既定） |
| `tako_orchestrator_projects` | 対応 | — | 実機セルフテスト: 項目 117（一時プロジェクトの登録と解除） |
| `tako_orchestrator_accounts` | 対応 | — | 実機実測: #937 の Windows 11 実測: `accounts list`（空）→ `add --inherit` → `show` → `list`（1 件）→ `remove` → `list`（空）の往復 |
| `tako_orchestrator_layout` | 対応 | — | 実機セルフテスト: 項目 72（master-reserved の配置と close 後のリフロー） |
| `tako_orchestrator_workers` | 対応 | — | 実機セルフテスト: 項目 105（レジストリの登録・再読込・再解決） |
| `tako_orchestrator_worker_status` | 対応 | — | 実機セルフテスト: 項目 74 / 105（IPC 応答と busy 中の後続 send）+ #877 で agents 経由の status=idle を実機実測 |
| `tako_orchestrator_respond` | 対応 | — | 実機セルフテスト: 項目 95 / 102 / 111（選択肢ダイアログの検知と番号 / ラベル確定） |
| `tako_orchestrator_report` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako orchestrator report --pane <p>` が source=scrollback で実ペインの出力を返す（第 1 層）。transcript 層は実機の claude が未認証で会話を作れず未実測 |
| `tako_orchestrator_run` | 対応 | — | 実機実測: #937 の Windows 11 実測: MCP `tako_orchestrator_run` が run_id を即返して worker を spawn し、CLI の同期版は status=timeout + 出力 + closed=true まで返す（完遂は実機の claude が未認証のため未実測） |
| `tako_orchestrator_run_status` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako orchestrator run-status` が starting/running → timeout/finished と elapsed_seconds を返す（MCP と CLI の両経路） |
| `tako_orchestrator_run_result` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako orchestrator run-result <run_id>` が status / duration_seconds / output / pane_id を返す |
| `tako_orchestrator_supervisor` | 対応 | — | 実機実測: #937 の Windows 11 実測: `supervisor status` → `set_mode --mode notify_only` → `status` → `set_mode --mode auto` の往復と `history --lines 5` |
| `tako_orchestrator_ledger` | 対応 | — | 実機実測: #937 の Windows 11 実測: spawn が作った台帳エントリを `ledger list` が返し、`ledger record --outcome pass` / `ledger amend` が反映され `ledger stats` が pass_rate=100 になる |
| `tako_limit_resume` | 対応 | — | 実機セルフテスト: 項目 111 / 117（オプトイン・ダイアログ型 / idle 型の出し分け・試行上限・プロファイル既定） |
| `tako_limit_service` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako limit-service` が現在サービスを返し、claude → codex → claude の切替が反映される |
| `tako_sessions` | 対応 | — | 実機実測: #877 で実機の session_id 解決（resolve_session_id_for_backend -> Some）を実測 + sessions の単体 14 本が実機で緑。resume のペイン起動そのものは未実測だが、経路は #867 で実機実測済みの launch と同じ |
| `tako_session_restart` | 一部対応 | 引き継ぎ再起動は使えるが、ハーネス更新（会話を保ったまま CLI を建て直す）はプロセスの終了要求が Windows 未対応のため使えない（#1067 / 境界 B5） | OS の仕様: tako_control::platform::process::terminate の Windows 実装は「プロセスの停止は Windows では未対応です」を返す（B5 の制御側が未実装）。handoff は queue_prompt_flow だけを使うので影響を受けない |
| `tako_task_gate` | 対応 | — | 実機テスト: acceptance_gates のゲート登録テストが実機で緑（落ちているのは execute_command の 5 件だけ） |
| `tako_task_gate_check` | 一部対応 | ゲートの登録と表示は動くが、コマンド型ゲートの実行が sh -c 決め打ちのため Windows では判定できない（#935） | 実機テスト: 実機の cargo test で execute_command 系 5 件が失敗（sh 不在）。PR / custom ゲートの判定は動く |
| `tako_task_gate_show` | 対応 | — | 実機テスト: acceptance_gates の表示テストが実機で緑 |
| `tako_task_checkpoint` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako task checkpoint --task-id … --phase running` が保存され、`tako task update --phase verifying` が反映される |
| `tako_task_list` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako task list --json` が保存したチェックポイントを issue / branch / project / prompt_head / phase つきで返す |
| `tako_task_resume` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako task resume <id> --tab <t>` が PowerShell 方言の env 前置き（`$env:TAKO_ORCHESTRATOR_ROLE=…; claude …`）でペインを立てる |

## git 連携

対応 14

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_git_log` | 対応 | — | 実機セルフテスト: 項目 85（git タブのセクション表示順。git データを取得できない環境ではこの項目は自己スキップする）+ #520 のパス可搬化と CRLF 耐性テストが実機で緑 |
| `tako_git_diff` | 対応 | — | 実機セルフテスト: 項目 85 / 79b（変更ファイルの分類と diff。git データを取得できない環境ではこの項目は自己スキップする）+ #520 の parse_diff CRLF 耐性 |
| `tako_git_show` | 対応 | — | 実機セルフテスト: 項目 85（コミット詳細。git データを取得できない環境ではこの項目は自己スキップする）+ #520 の to_git_path / repo_relative |
| `tako_git_stage` | 対応 | — | 実機セルフテスト: 項目 79b（ステージング UI の分類とコミット挙動。git データが取れない環境では自己スキップする項目） |
| `tako_git_unstage` | 対応 | — | 実機セルフテスト: 項目 79b（ステージング UI の分類とコミット挙動。git データを取得できない環境ではこの項目は自己スキップする） |
| `tako_git_commit` | 対応 | — | 実機セルフテスト: 項目 79 / 79b / 86（コミットメッセージ入力欄・両経路のコミット・IME。git データを取得できない環境ではこの項目は自己スキップする） |
| `tako_git_conflicts` | 対応 | — | 実機セルフテスト: 項目 82b / 109（使い捨てリポのコンフリクトを git パネルが認識する。git データを取得できない環境ではこの項目は自己スキップする） |
| `tako_git_resolve_agent` | 対応 | — | 実機セルフテスト: 項目 82b / 109（3 択の開閉と起動。git データを取得できない環境ではこの項目は自己スキップする）+ #867 でエージェントペインの実起動を実機実測 |
| `tako_git_checkout` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako git checkout topic --pane <p>` が checked_out=true を返し実リポジトリの HEAD が移る（cwd が通常形のペインで実施 = #970） |
| `tako_git_branch_create` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako git branch <名前> --pane <p>` が新ブランチを作ってチェックアウトする（cwd が通常形のペインで実施。verbatim prefix の cwd では解決できない = #970） |
| `tako_git_merge` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako git merge topic -y --pane <p>` が merge コミットを作り、`-y` なしのドライランは作業ツリーを変えずに予測（predicted_conflicts）だけ返す |
| `tako_git_merge_abort` | 対応 | — | 実機実測: #937 の Windows 11 実測: 衝突する merge が conflicted=true / conflicts=[c.txt] になり、`tako git abort` が aborted=merging を返して HEAD と作業ツリーが元へ戻る |
| `tako_git_pull` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako git pull --pane <p>` が対向 bare リポジトリの新コミットを取り込み merge コミットを作る |
| `tako_git_push` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako git push --pane <p>` で対向 bare リポジトリの main が push 後の HEAD へ進む |

## 永続化とセッション

対応 6・未対応 / 未実測 2

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_persist` | 対応 | — | 実機セルフテスト: 項目 58（tako persist の ON/OFF と状態取得）+ 実機 psmux_backend 16/0 |
| `tako_logs` | 対応 | — | 実機セルフテスト: 項目 87 / 104（ペインログのクローズマーカーと発生源） |
| `tako_tmux_list` | 対応 | — | 実機実測: #866 の製品経路 A/B: 項目 48 が既定で通過（TAKO_866_KEEP_EXACT_TARGET=1 では FAILED） |
| `tako_tmux_kill` | 対応 | — | 実機実測: #866 の製品経路 A/B: 項目 48 で対象だけが消え、隣の tako-test2 が残ることまで実測 |
| `tako_tmux_open` | 未対応 / 未実測 | 永続化の器は psmux で、attach と send-keys を前提にする tmux 操作は動かない | 実機実測: セルフテスト項目 68 / 73 は attach / send-keys 前提のため psmux ではスキップ |
| `tako_tmux_select_window` | 対応 | — | 実機実測: #937 の Windows 11 実測: 2 つ目の window を作ってから `tako tmux select-window 0 / 1 --pane 1` でアクティブ window が実際に切り替わる |
| `tako_tmux_cleanup` | 対応 | — | 実機実測: #937 の Windows 11 実測: 器へ孤児セッションを 1 つ作ると `tako tmux cleanup` が killed=[tako-orphan937] を返し、使用中の 5 セッションには触らない |
| `tako_tmux_resize` | 未対応 / 未実測 | psmux がセッションの寸法指定（-x / -y）を反映しないため寸法を変えられない | 実機実測: psmux が -x / -y を受け取っても反映しないことを #866 の調査で確認 |

## OS 連携

対応 5・一部対応 2・対象外 1

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_file_op` | 対応 | — | 実機実測: #617 の Windows 11 実測: 空白 + 日本語名 / 読み取り専用 / ディレクトリ / 315 文字のパスがいずれも復元可能な状態でごみ箱へ入り、reveal で対象が選択され、既定アプリが起動する。実機で緑のテスト: os_integration の windows モジュール（FOF_ALLOWUNDO のフラグ構成 / 絶対化 / /select, の形） |
| `tako_sleep_guard` | 対応 | — | 実機実測: powercfg /requests の SYSTEM に tako のアサーションが出て mode=off で消える。蓋閉じは lid-guard.json の生成まで確認 + セルフテスト項目 120 / 121 |
| `tako_port_detect` | 対応 | — | 実機実測: スライス 9 で tako list が 8123/node.exe を拾い、psmux の偽 listen 21 個を 1 つも報告しない + セルフテスト項目 55（ON/OFF） |
| `tako_fda` | 対象外 | Windows に macOS の TCC（フルディスクアクセス）に相当する仕組みが無い | OS の仕様: Windows に TCC（フルディスクアクセス）相当の仕組みが無いので許可を求める対象が無い（#515 の判定テストが固定） |
| `tako_shell_integration` | 一部対応 | cwd 追従とコマンド状態は器（psmux）越しでも側路で届くが、psmux が OSC を素通ししないため status の effective は false のままになる（#766） | 実機実測: #766 で側路の state が unknown → idle、exit_code=3、cwd が OSC 7 由来で追従。実機 shell_integration_powershell 7/0（#525） |
| `tako_stale_binary` | 一部対応 | PATH 上の claude の実在確認は動くが、実行中の claude のパスを解決できないため古いバイナリの警告が出ない（#936 / #726） | 実機テスト: stale_binary::tests::test_pidpath_self と ランチャ探索…の 2 件が失敗。PATH 上の探索は #898 で境界 B16 へ寄せて実機実測済み |
| `tako_check_health` | 対応 | — | 実機実測: #937 の Windows 11 実測: MCP `tako_check_health` が HTTP 200 で healthy=true / tmux_available=true / persist_enabled=true / version_match=true / issues=[] を返す |
| `tako_telemetry` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako telemetry status` → `on` → `status`（true）→ `off` → `status`（false）の往復 |

## セットアップと設定

対応 10・一部対応 1・未対応 / 未実測 1

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_setup` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako setup --check` が claude（未認証）/ psmux / git / tailscale / スリープ防止 / MCP 未登録を正しく列挙し、`--changes --json` が revision 17 と未適用一覧を返す。対話の通し（エージェント起動）は実機の claude が未認証のため未実測 |
| `tako_setup_bootstrap` | 対応 | — | 実機実測: #1057 の Windows 11 実測: 隔離 USERPROFILE + PATH 剥ぎで `tako setup` が install（install.ps1 を -ExecutionPolicy Bypass -File で実行）→ path（ユーザー環境変数 Path へ追記・undo-path で完全復帰）→ auth 誘導 まで到達。2 回目は無言で素通り |
| `tako_setup_changes` | 対応 | — | 実機テスト: changes.yaml の連番・platforms 絞り込みテストが実機で緑（#525 が platforms: を最初に使う） |
| `tako_setup_deps` | 一部対応 | 依存の検出はできるが、導入の実行代行は macOS（Homebrew）だけ。Windows は winget のコマンドを案内する | 実機実測: #1057 の Windows 11 実測: `tako setup deps` が器（psmux）/ git / tailscale を実際の解決結果つきで列挙し、install は winget を代行せず not_delegable で理由 + コマンドを返す |
| `tako_setup_mcp` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako setup-mcp` が claude の設定（スクラッチ HOME 側）へ tako を登録し旧内容を backups へ退避する。別途、実 HOME の登録に対して `claude mcp list` が `tako.exe mcp serve` を Connected と健康判定したので、stdio ブリッジ自体も Windows で通る |
| `tako_setup_models` | 未対応 / 未実測 | 実装はプラットフォーム共通で macOS と同じ経路を通るが、Windows 実機での実測がまだ無い（動く見込み。失敗したらまずここを疑う） | 未実測 |
| `tako_settings` | 対応 | — | 実機セルフテスト: 項目 96 / 120（プロファイルタブ・スリープ防止タブの表示構成） |
| `tako_migrate` | 対応 | — | 実機セルフテスト: 項目 123（実 dispatch で自動マイグレーション）+ migrations の単体 20 本が実機で緑 |
| `tako_agents_sync_rules` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako agents sync-rules --source <正本>` が claude のグローバル指示へマーカーブロックを書き（action=updated + .bak 生成）、未導入の codex / agy は理由つきで skip される |
| `tako_config_share` | 対応 | — | 実機実測: #937 の Windows 11 実測: `tako config init` が共有リポジトリを作って初回コミット（7 ファイル）、`tako config` が差分（same 4）を出し、`tako config pull` が 1 件取り込む |
| `tako_platform` | 対応 | — | 実機テスト: platform_parity 13 本と support の単体が実機で緑（判定は純粋関数） |
| `tako_agent_support` | 対応 | — | 実機テスト: agent_parity 5 本と agent_support の単体 15 本が緑（判定は純粋関数で OS を見ない） |

## リモートアクセス

対応 1・一部対応 2・未対応 / 未実測 8

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_remote_start` | 未対応 / 未実測 | remote デーモンの起動・停止に unix 前提の処理が残っており、Windows 実機での通し確認も未了 | 実機テスト: 実機の cargo test で remote::tests の 2 件（daemon_stop_impl / is_process_alive）が失敗 |
| `tako_remote_stop` | 未対応 / 未実測 | remote デーモンの起動・停止に unix 前提の処理が残っており、Windows 実機での通し確認も未了 | 実機テスト: 同上（daemon_stop_impl はpid再利用時にkillしない が失敗） |
| `tako_remote_status` | 未対応 / 未実測 | #1038 で serve の中継先をループバック TCP へ変えたので、`unix socket serve target is not supported on Windows` で止まる原因は無くなった。ただし Windows 実機での通し（setup の 4 段目 → デーモン起動 → スマホからの接続）は未実測（#971） | 実機実測: #937 の Windows 11 実測: `tako remote status` は running=false を返すが、デーモンを起動できないので常にこの状態（#971） |
| `tako_remote_setup` | 未対応 / 未実測 | #1038 で serve の中継先をループバック TCP へ変えたので、`unix socket serve target is not supported on Windows` で止まる原因は無くなった。ただし Windows 実機での通し（setup の 4 段目 → デーモン起動 → スマホからの接続）は未実測（#971） | 実機実測: #937 の Windows 11 実測（#1038 の修正**前**）: 1〜3 段（Tailscale 検出 / ログイン / HTTPS 証明書）は OK で、4 段目の serve 設定が `unix socket serve target is not supported on Windows` で失敗した。#1038 でこの原因は取り除いたが、実機での再測はまだ（#971） |
| `tako_remote_devices` | 未対応 / 未実測 | #1038 で serve の中継先をループバック TCP へ変えたので、`unix socket serve target is not supported on Windows` で止まる原因は無くなった。ただし Windows 実機での通し（setup の 4 段目 → デーモン起動 → スマホからの接続）は未実測（#971） | 実機実測: #937 の Windows 11 実測: `tako remote devices list` は running=false の形を返すが、デーモンを起動できないので端末を登録できない（#971） |
| `tako_remote_agents` | 未対応 / 未実測 | #1038 で serve の中継先をループバック TCP へ変えたので、`unix socket serve target is not supported on Windows` で止まる原因は無くなった。ただし Windows 実機での通し（setup の 4 段目 → デーモン起動 → スマホからの接続）は未実測（#971） | 実機実測: #937 の Windows 11 実測: `tako remote agents` は agents=[] を返し走査そのものは動く（#877 の境界）が、`tako remote setup` が serve 設定で失敗しデーモンを起動できない（#971） |
| `tako_remote_messages` | 未対応 / 未実測 | remote デーモンの起動・停止に unix 前提の処理が残っており、Windows 実機での通し確認も未了 | 実機実測: #937 の Windows 11 実測: CLI が <SESSION_ID> を要求するところまで確認。実機の claude が未認証で会話を作れないため本体は未実測（デーモン側は #971 でブロック） |
| `tako_remote_scrollback` | 未対応 / 未実測 | スクロールバックの取得が器の境界を通らず psmux で解決できない（セッション名でもペイン ID でも `no server running` になる。#972） | 実機実測: #937 の Windows 11 実測: セッション名でもペイン ID でも `psmux: no server running on session '<socket>__<target>'` になる。同じソケットへ境界経由で叩く `tako tmux list` は成功する（#972） |
| `tako_open_remote` | 一部対応 | Windows の OpenSSH は接続多重化（ControlMaster）に対応しないため、操作ごとに独立した SSH 接続になる。鍵・ssh-agent で入れる相手は変わらないが、パスワード認証しか無い相手はツリーの展開やファイルの取得のたびに認証が要る。接続が生きているかもソケットで判定できないので、切断後の自動再接続（#1040）も armed にならない（#1090） | 実機実測: #1090 の Windows 11 実測（OpenSSH_for_Windows_10.0p2）: ControlMaster 系を渡すと接続の前に `getsockname failed: Not a socket` / exit -1 で死に、渡さないと同じ相手へ exit 255（`Could not resolve hostname` / `Host key verification failed`）まで進む。渡さない形にしたうえで到達不能ホストの 3 経路（split / tab / pane）が理由 + 次の一手を出してローカルのシェルへ戻ることを実測 |
| `tako_remote_folder` | 一部対応 | 同梱の OpenSSH クライアントで開けるが、接続多重化（ControlMaster）が無いので操作ごとに認証が起きる（パスワード認証しか無い相手は展開のたびに聞かれる。接続が生きているかも判定できない。#1090）。ペインの ssh を検知した自動追加（#976）は、プロセスのコマンド行を採れないので働かない（明示的に開く経路だけが使える） | 実機実測: #1090 の Windows 11 実測: ControlMaster 系を渡した sftp は `getsockname failed: Not a socket` で握手にすら進まないが、渡さないと同じ相手へ SSH の握手が進む（`Host key verification failed` まで到達）。渡さない形で `tako remote-folder open` / `ls` が実 SSH 先の一覧を返すことを実測 |
| `tako_ssh_hosts` | 対応 | — | 実機テスト: ~/.ssh/config の解析は純粋関数で、remote_fs / ssh_hosts の単体が実機で緑（ホーム解決は #870 で一本化） |

## アップデート

一部対応 1

| 機能 | 状態 | 差分 | 根拠 |
| --- | --- | --- | --- |
| `tako_update` | 一部対応 | 更新の確認とリリースノートの表示は動くが、更新の適用（インストーラーの実行と再起動）は Windows 実機で未実測（#937） | 実機セルフテスト: 項目 90（更新画面と Markdown のリリースノート）+ #587 / #723 で実機の配布物生成とバージョン解析を実測。適用そのものは未実測 |

## この表の作り方

正本は `crates/tako-core/src/platform/support.rs` の対応マトリクスです。
判定を変えるときは根拠（実機セルフテストの項目・実機で緑のテスト名・実測の記録）を
同時に書く必要があり、書かずに「対応」へ倒すとテストが落ちます。

このページの再生成と同期検査は次のとおりです。

```sh
cargo build -p tako-cli
node scripts/gen-windows-support-docs.mjs          # 再生成
node scripts/gen-windows-support-docs.mjs --check   # 同期検査
```
