# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> 直近の作業のみ参照、エントリ 30 件超 or 14 日より古いものは `progress-archive.md` への移送を提案する。
> 自動削除はしない。常にユーザー確認を経る。

## 追記フォーマット

```markdown
## YYYY-MM-DD
- {一行サマリ。何を/どこを/結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

> 30 件超のため 2026-06-11〜06-13 の 30 エントリを `progress-archive.md` へ移送済み（2026-06-16）

## 2026-06-14（プレビュー書式崩れ 4 件一括修正）

- ① コード行を行番号（固定幅左列）+ 本文（flex_1 折り返し）に分離 → 長行見切れ・MD コード
  ビュー行重なり解消 ② UI 経路で pending_highlights 未 drain → syntect ハイライト未発火を
  修正 ③ MD ListItem を flex_1 div で折り返し ④ ①と同根
- 関連コミット: `83cfe2c` `[修正] プレビュー表示の書式崩れ4件を一括修正`
- 次: ユーザー再起動 → 実機確認

## 2026-06-14（tmux パネル UI 刷新）

- render_tmux_view / render_kill_confirm を全面書き換え: アコーディオン折りたたみ、
  状態色統一（緑/黄/赤）、hover 時のみ × 表示、GPUI text_ellipsis、パディング削減、
  セクション区切り明確化。FileOp dispatch（FR-3.12）も同時追加
- 関連コミット: `20261e9` `[改善] tmux パネル UI 刷新 + FileOp dispatch`
- 次: ユーザー再起動 → 実機確認

## 2026-06-14（ファイルツリーのコンテキストメニュー + D&D パス挿入）

- FR-3.12 コンテキストメニュー（右クリック→ パスコピー / Finder 表示 / cd / リネーム /
  新規ファイル・フォルダ / ゴミ箱。dispatch `FileOp` + CLI `tako file` + MCP
  `tako_file_op` = 計 23 ツール。開発不変条件）+ FR-3.13 D&D パス挿入（ツリーから
  ターミナルペインへドロップでパス文字列を send。プレビューペインなら FR-3.11 維持）
- 関連コミット: `e11b2be` `[機能追加] ファイルツリーの右クリックメニュー + D&D パス挿入`
- 次: ユーザー再起動 → 実機確認 / インラインリネーム UI の実装（構造は準備済み）

## 2026-06-14（インラインテキスト入力 UI）

- FR-3.12 の残りを完了: ファイルツリーのコンテキストメニューから名前変更・新規ファイル・
  新規フォルダを選ぶとインライン入力欄が表示される。Enter で確定（dispatch FileOp）、
  Esc でキャンセル。IME 入力対応（EntityInputHandler の振り分け）。カーソル移動・
  BS/Del サポート。新規時は親ディレクトリ自動展開（FileTree::expand_dir 追加）。
  MCP ツール数セルフテスト期待値を 23 に修正
- 次: ユーザー再起動 → 実機確認

## 2026-06-14（パフォーマンスバグ修正 2 回目: tmux ポーリング非同期化）

- 2 秒ポーリングの `refresh_tmux_data` が 6 回の同期 tmux サブプロセス（計 25〜50ms）で
  UI スレッドをブロック → background executor に移行（UI 占有 < 0.1ms）。
  TmuxOpen の存在確認も `has_session`（1 コマンド）に軽量化
- 関連コミット: `24eecec` `[改善] tmux ポーリングの非同期化で UI スレッドブロック解消`
- 次: ユーザーが tako 終了 → .app 差し替え → 再起動して実機確認

## 2026-06-14（git タブ実装: FR-3.6 git graph + FR-3.9 diff ビューア）

- `tako-core::git` 新設（git CLI 子プロセス、tmux.rs と同パターン。log/branch/status/diff
  パーサ + unit test 5 本）。dispatch `GitLog`/`GitDiff` + CLI `tako git log/diff` + MCP
  `tako_git_log`/`tako_git_diff`（計 25 ツール）。右パネルの git ビューをプレースホルダから
  4 セクション（ブランチ / 変更ファイル / コミットグラフ / diff）のアコーディオンに置き換え。
  cwd 連動 2 秒ポーリング + パネル開いた瞬間の即時 fetch。コミットクリックでそのコミットの diff 表示
- cargo test 93 pass・clippy / fmt 緑。feature/git-tab ブランチ
- 次: ユーザーが .app 差し替え → 再起動して実機確認

## 2026-06-14（たまり場機能 FR-2.15 完成）

- FR-2.15 全 5 要件を実装: × ボタンを kill → たまり場退避に変更 / ドロワー UI
  （ステータスバー「⏏ 退避」ボタン、下部展開、状態色 + ラベル + 確認付き kill）/
  ドロワーからペインエリアへ D&D 復帰 / CLI・MCP 4 操作（計 27 ツール）/
  layout.json 永続化（shelved フィールド追加、serde default で後方互換）
- 次: ユーザー再起動 → 実機確認

## 2026-06-15（タブ単位退避: 最小化ボタン + D&D 退避）

- タブバーに最小化ボタン（ー）を追加、タブ内全ペインをまとめてたまり場へ退避。
  タブを退避ボタン/ドロワーへ D&D でも退避可。コアに `shelve_tab()` 新設（テスト 2 本）
- 関連コミット: `d55be1e` `[機能追加] タブ単位の退避: 最小化ボタン + D&D 退避`

## 2026-06-15（tmux orphan 修正 + tmux ビュー退避セクション）

- TmuxOpen ペインの close 時に tmux セッションが kill されず orphan 化する問題を修正。
  `drop_tmux_view_session()` 新設で remove_pane/tab/detach_session/shelved kill 全経路を修正。
  tmux パネルに「⏏ 退避中」セクション追加（状態ドット + 復帰ボタン）
- 関連コミット: `a37812e` `[修正] tmux D&D タブの orphan 化防止 + tmux ビューに退避中セクション追加`
- 次: ユーザー再起動 → 実機確認

## 2026-06-15（tmux ビューの二重表示 / 無限ネスト / orphan 根治）

- 二重表示 + 退避ラベル（`b9584af`）: shelved backend を kill漏れ?から除外、退避ラベルを
  pane ID → cwd ベース名へ
- ラッパー orphan 根治: `TmuxViewTarget` を `session`（元・監視/再 attach）と `wrapper`
  （`tako-view-*`・close 時 kill）に分離。`drop_tmux_view_session` はラッパーのみ kill
  （`tako-view-` 接頭辞ガード）。旧実装の「元セッション名登録」= ①ラッパー orphan
  ②実セッション誤 kill の二重バグを解消
- 無限ネスト根治（`tako-view-tako-view-...`）: `TmuxOpen` で tmux `session_group` へ正規化 +
  `tako-view-*` の開き直しは新ラッパーを作らず元を直接 attach（`dispatch.rs`）
- orphan 一括クリーンアップ（FR-2.16.11）: 起動時自動 + dispatch `TmuxCleanup` + CLI
  `tako tmux cleanup` + MCP `tako_tmux_cleanup`（計 31 ツール）。backend socket 上の
  `tako-`・detached・非 grouped・protected 外のみ kill（grouped/attached/実セッションは不可侵）
- 検証: clippy 緑 / cargo test 全緑 / セルフテスト = PDF（既知）以外緑。stale だった
  セルフテスト 2 件（ツール数 29→31・× ボタン kill→退避）も修正
- 次: ユーザー再起動（`build-app.sh --install`）→ 実機確認

## 2026-06-15（×=kill バグ修正 + 退避エリア横並びプレビュー化）

- 緊急修正: ペインの × が `shelve_pane` だけ呼び tmux セッションを kill せず「管理外」化
  していた問題を、× → `remove_pane`（両セッション kill 込み）へ統一して根治。ペイン単位の
  退避は新設 ー ボタン（`shelve_pane_button`）へ分離（タブの ー/× と一貫）
- 退避 UI 刷新: たまり場ドロワーを縦テキスト → 横並びの実画面プレビューカード + 横スクロール
  へ。`terminal_screen_lines()` を render_pane と共用しサムネイル描画、各カードにタイトルバー
  （状態ドット + ラベル + 復帰 + 右上 × kill）。既定高さ 150→240px
- 関連コミット: `da26023` `[スタイル]` / `16066b5` `[修正] ×で tmux kill` /
  `9791b6a` `[改善] 退避エリア横並びプレビュー化`
- 検証: clippy / fmt 緑、セルフテスト PDF（既知）以外緑（47/47b/47c 追加・通過）
- 次: ユーザー再起動（`build-app.sh --install` 実行予定）→ 実機確認

## 2026-06-15（サイドバー tmux ビュー一本化: 二重化解消・表示分類・退避のタブ別分離）
- 統合 tmux ビュー + 退避を 3 改修。①attach 外部セッションをホストペイン行下へ入れ子化し
  二重表示解消（FR-2.16.9 統合）②各ペイン行に表示中/バックグラウンドのバッジ + list に
  surface 公開（FR-2.16.12）③`Workspace::shelved` を `Vec<ShelvedPane>`（由来タブ記録）へ
  し、タブツリー/ドロワーを由来タブ別に分離・閉じたタブは集約・復帰は由来タブへ（FR-2.15.6）
- ShelvedList に origin_tab/surface 公開、unshelve は target 省略で由来タブ復帰、layout 永続化
  に origin_tab 追加（後方互換）。dispatch/layout/workspace のテスト追加
- 関連コミット: `f64d2a3` `[改善] サイドバー tmux ビュー一本化…`
- 検証: build / clippy(-D warnings) / fmt / test 全緑、セルフテストは既知 PDF 以外緑
- 次: push → `build-app.sh --install` → 再起動で実機確認

## 2026-06-15（タブツリー ホバー/ピン プレビュー 4 機能）
- F1 ホバープレビュー（FR-2.16.13）: バックグラウンド行を on_hover でマウス位置に実画面
  サムネイル（terminal_screen_lines・リサイズせず）ポップアップ。ライブ更新は on_term_event 由来
- F2 折りたたみ改修（FR-2.16.14）: 意味論を「バックグラウンド行+退避だけ隠す」へ（Q2 選択）。
  TabId キー化 + Request::CollapseTab + tako collapse + MCP + list collapsed + layout 永続化
- F4 グループプレビュー（FR-2.16.16）: 閉じたタブグループを on_hover で全退避ペイン縦積み。
  PreviewTarget::ClosedGroup 追加。閉じタブ分割自体は f64d2a3 で実装済み
- F3 ピン留め（FR-2.16.15）: 📌 でアプリ内フローティング窓化（OS マルチ窓不使用）。D&D 移動 +
  × 解除 + ライブ。Request::Pin + tako pin + MCP tako_pin_preview + list pinned（計 33 ツール）
- 関連コミット: `765af0d`(F1) `cf04a31`(F2) `96d01b3`(F4) `c12d4c5`(F3)
- 検証: build / clippy(-D warnings exit 0) / fmt / test 全緑（app33/cli10/control58/core103）。
  セルフテストは既知 PDF 以外緑 = ツール数 33 通過
- 次: push → tako 終了（Cmd-Q）→ `build-app.sh --install` → 再起動で実機確認

## 2026-06-16（GitHub Releases 配布整備）
- `scripts/release.sh` 新設: build-app.sh → ditto zip → gh release create の一発スクリプト。
  README にダウンロード・インストール手順（Gatekeeper 対処法）追加。AGENTS.md コマンド追加
- 関連コミット: `8c0ce17` `[機能追加] GitHub Releases 配布用スクリプト + README ダウンロード手順`
- 次: ユーザー判断で `scripts/release.sh --publish` または `--draft` で初回リリース作成

## 2026-06-16（tmux window タブツリー統合）
- バックエンドセッション内の tmux window をサイドバー tmux ビューに表示。子 worker が
  `tmux new-window` で作った window が見える + クリック切替 + ホバープレビュー + ピン留め。
  `TmuxSelectWindow` dispatch + CLI `tako tmux select-window` + MCP `tako_tmux_select_window`（計 34 ツール）
- 関連コミット: `f25373f` `3c250b8`
- 検証: build / clippy / fmt / test 全緑
- 次: `build-app.sh --install` → 再起動で実機確認

## 2026-06-18（オーケストレーター機能の完全内蔵）
- tako にオーケストレーター機能を内蔵。外部スクリプト依存ゼロで `tako master` でマスター起動、
  MCP 3 ツール + CLI 5 コマンド + デフォルト system prompt 埋め込み + docs（計 40 ツール）
- 関連コミット: `6d764d7` `b68f190` `243dae6` `c27f5e5`
- 検証: build / clippy(-D warnings) / fmt / test 全緑

## 2026-06-23（MCP/IPC 再起動耐性の強化）
- IPC ソケットを固定パス化（`<data_dir>/tako.sock`）、トークンを永続化（`<data_dir>/token`）、
  persist ON 時の ⌘Q で discovery cleanup を省略。再起動後も同じソケット・トークンで再接続可能に
- 関連コミット: `8037055` `[改善] MCP/IPC の再起動耐性を強化`
- 検証: build / clippy(-D warnings) / test 全緑

## 2026-06-24（main.rs 大規模リファクタリング — モジュール分割）
- main.rs（13,736行）から 7 モジュールを分離し 8,359 行へ（39% 削減）。機能変更なし
- 分離: keybindings / tab_bar / status_bar / drawer / sidebar / right_panel / preview_render
- 関連コミット: `d0e2eda` `3baca26` `066c4df`
- 検証: build / clippy(-D warnings) / fmt / test 全緑

## 2026-06-24（コードベース品質改善 — 6コミット一括）
- dead code 削除（preview_md_block 94行 / is_pdf_path / is_video_path をテスト内へ移動）
- unwrap 除去 4箇所（preview_render / sidebar / main.rs の防御的フォールバック）
- コード重複共通化: resolve_bin()（git/tmux 89→25行）、cleanup_state_files()（remote.rs 4箇所）、
  PreviewState::error()（7箇所）、dir_of() / validate_name()（dispatch.rs 3箇所ずつ）、
  pretty_json()（CLI 9箇所）
- 関連コミット: `fa0428a` `ebbdb6e` `5a56f47` `8b0b3c3` `7efa810` `e31f27c`
- 検証: 各コミットで build / clippy(-D warnings) / fmt / test 全緑

## 2026-06-25（UI 描画パフォーマンス最適化 — 5項目）
- on_term_event の notify を 16ms デバウンス、sync_filetree_roots を render 毎フレームからイベント駆動に移行、
  terminal_screen_lines のスタイルラン検索を O(N+M) 化 + HL キャッシュ、filetree.rows() キャッシュ化、
  screen snapshot のグリッドをフラット配列化
- 関連コミット: `e4abc98` `4884630`
- 検証: build / clippy(-D warnings) / fmt / test 全緑。build-app.sh --install 済み

## 2026-06-25（orchestrator spawn に tab パラメータ追加）
- `tako_orchestrator_spawn` に `tab` パラメータ追加。指定タブのフォーカスペインを分割元にする。
  複数マスター運用時の出力先タブ明示指定が可能に。優先順位: pane > tab > master role 検索
- 関連コミット: `dc4b65c` `[機能追加] tako_orchestrator_spawn に tab パラメータを追加`
- 検証: build / clippy(-D warnings) / fmt / test 全緑。build-app.sh --install 済み

## 2026-06-25（cleanup_orphan_tmux 二重起動ガード追加）
- dev build 起動時に production の全 tmux backend セッションを誤 kill する事故を防止。
  `ports::other_tako_running()` を tako-core に追加し、`cleanup_orphan_tmux` 冒頭でスキップ
- 関連コミット: `8b81e48` `[修正] cleanup_orphan_tmux に二重起動ガードを追加`
- 検証: build / clippy(-D warnings) / fmt / test 全緑。build-app.sh --install 済み

## 2026-06-25（spawn 信頼性 + セッション追跡の改善）
- 4項目: ①複数 master の suffix マッチ ②`spawned_by` フィールド追加 ③`worker_status` shelved 対応 ④dead code 除去
- 関連コミット: `53520eb` `56f55eb` `7a0126f`
- 検証: build / clippy(-D warnings) / fmt / test 全緑（109 passed）

## 2026-06-26（spawn TAKO_PANE_ID stale 問題の根治）
- `new-session -e` で TAKO_PANE_ID/TAB_ID を直接注入。旧 `set_pane_env`（セッション未存在時に呼ばれ常に no-op）を除去
- 関連コミット: `0156b9a` `[修正] spawn 配置: TAKO_PANE_ID を new-session -e で直接注入し stale ID 問題を根治`
- 検証: build / clippy(-D warnings) / fmt / test 全緑（109 passed）

## 2026-07-02（Issue #23 フェーズ A: リモート接続基盤・バックエンド API）
- WS 画面プッシュ（tiny_http upgrade + tungstenite、認証は Sec-WebSocket-Protocol）、
  screen API の ANSI/cursor/lines、resize API（+ `tako tmux resize` + MCP）、connect URL の
  token fragment 化、/api/agents（pid 祖先辿りで pane 対応付け）+ /api/sessions/:id/messages
  （transcript 正規化。tako-control::agents / transcript 新設）、リレー URL 統一 + close ハンドラ。
  CLI `tako remote agents/messages` + MCP 3 ツール追加（計 48）
- 関連コミット: `d66a7e7` `4496109` `4b94e68` `f3edb4b` `3e1c296` `61c0fe9`
- 検証: build / clippy(-D warnings) / fmt / test 全緑 + 実デーモンで HTTP/WS e2e（401/101/差分プッシュ/resize 実寸/close 消滅）
- 次: フェーズ B（フロント刷新）は別 worker。引き継ぎは activeContext.md 参照

## 2026-07-02（Issue #27 ホットフィックス: 既定プロファイル [1m] ハードコード廃止 → v0.2.4）
- 既定プロファイルを model 無指定（claude CLI 既定）化。旧既定値 `claude-opus-4-6[1m]` は
  起動時（master / setup / spawn）に自動マイグレーション（backup-1m 付き）、明示 opt-in は警告のみ。
  config.yaml の死に設定 master_model/worker_model/effort を廃止し profiles/*.yaml に一本化
  （setup アシスタントの「Pro に 1M 推奨」誤りと書き先も修正）。
  `tako orchestrator profiles list/show/set` + MCP `tako_orchestrator_profiles`（計 49 ツール）新設
- 検証: build / clippy(-D warnings) / fmt / test 全緑 + スクラッチ HOME で実バイナリの
  マイグレーション・profiles CLI e2e（セルフテストは別 worker と競合するため未実施、ツール数のみ更新）
- 次: squash merge → v0.2.4 リリース（release.sh --publish + homebrew-tako cask 更新）

## 2026-07-02（Issue #32: プロンプト送達の確認ループ化）
- spawn / send の送達を検証付きに: `tako-control::claude_tui` 新設（実 TUI 採取画面での状態
  検出 + `~/.claude.json` 事前信頼 + tmux 送達確認配送）、PromptFlow 刷新（信頼ダイアログ
  承諾 → bracketed paste → 分離 Enter → 入力欄空検証 + Enter 再送、同一ペイン直列化）、
  Send は alt_screen で送達フロー経由に。tmux 3.6 で `=session` target-pane が解決不能な
  既存バグ（send/read フォールバック全滅）も `=session:` 化で修正
- 検証: build / clippy(-D warnings) / fmt / test 全緑（+12 unit）+ 実 claude E2E 3 本
  （未信頼フォルダ spawn / 事前信頼 / 長文マルチライン。`claude_tui_e2e --ignored`）

## 2026-07-03（Issue #30: タブ永続化の根治 — tmux 依存除去 + PTY 死亡の防御 + 診断）
- 根因 1: 保存・復元の両方が `tmux_backend::available()` にゲートされ、tmux 無し（Homebrew
  配布先）では layout.json が一度も書かれず無音で全タブ消失。ゲート除去で「tmux 不在 =
  構造のみ永続化（復元は保存 cwd の新シェル）」へ
- 根因 2（2026-07-03 実機で全タブ消失）: PTY 死亡（tmux サーバー外部 kill・クライアント kick）を
  明示 close と同一視し、バックエンドセッション kill + layout.json 削除で全損。`CloseReason`
  （Explicit/Exited）を導入し、Exited ではセッション kill も layout 削除もしない
- 診断: `<data_dir>/persist.log`（復元成否・理由・明示削除。256KB ローテート）、破損ファイルの
  `.corrupt` 退避、`tako persist` / MCP に layout_path / layout_exists / last_restore / log_path
- 検証: release .app + クリーン HOME で e2e 3 ラウンド（tmux 不在復元 / tmux 完全復元 /
  サーバー外部 kill → layout 保持 → 復元）
- 次: なし（#30 クローズ）

## 2026-07-03（セルフテスト回復: 6/23 以来の壊れ 7 件修正 + MCP 応答破損の根治）
- Issue #23 フェーズ A の検証中にセルフテストが 6/23 から壊れ続けていたのを発見・修正:
  ①split フォーカス仕様変更（3c9d363）への追従漏れ 5 箇所（項目 18/40/41b/47/47b/63 に --focus 付与）
  ②MCP HTTP 応答が 32KB 超で chunked になりマルチバイト境界で壊れる問題（48 ツール化で顕在化 →
  Content-Length 固定 + 回帰テスト）③タイミング検査 3 件のリトライ化（17/40b/46）
- 関連コミット: `79c3272` `7d71d84` `1801509` `d3f75a1`
- 残: 項目 46「全角行のクリック」が決定的に失敗（GUI 座標解決。#23 と無関係、要調査 → activeContext）

## 2026-07-03（Issue #28: Shift+Enter 改行の根治）
- 根因 = 修飾キーの CSI u 送出が tmux バックエンドペイン限定で、tmux 無し環境
  （Homebrew cask は tmux 非依存 = 配布先の既定）の直接 spawn ペインが素の \r に潰れていた。
  claude は kitty 未要求でも CSI u を解釈（v2.1.198 素の PTY 実測）→ ModifiedOnly を全ペイン
  既定化（CsiUMode::Off 廃止）。setup prompt に「キーバインド変更をしない・案内しない」を明記。
  セルフテスト 45b（GUI キー経路で CSI u 着弾）+ 45c（TAKO_SELF_TEST_CLAUDE=1 で実 claude 改行 e2e）追加
- 関連コミット: `[修正] tmux 無し環境で Shift+Enter 改行が効かない問題を根治 (#28)`（fix/28 worktree → PR squash merge）
- 次: Homebrew 配布ビルド更新後にテスター実機で最終確認

## 2026-07-03（Issue #36: アプリ内更新の配布系統自動判別 + 更新前確認 + 自動再起動）
- 配布系統自動判別（Caskroom パス判定）、更新前確認ダイアログ（プロセス消失警告）、
  更新完了後の自動再起動（layout 保存 → open -n → quit）、PATH 上の CLI 重複検知。
  Request::Update + CLI `tako update status/check/apply` + MCP `tako_update`（計 50 ツール）
- 関連コミット: `942d692`（worktree → PR #40 squash merge）
- 次: なし（#36 クローズ）

## 2026-07-03（Issue #42 + #26: リモートフロントエンド二層構成刷新）
- 二層構成で PWA 全面刷新: 履歴レイヤー（scrollback API + クライアント描画 + スクロール/コピー）+
  ライブ画面レイヤー（REST→WS 移行 + 接続時自動リサイズ + 切断時リセット）。input API に keys
  フィールド追加。textarea 化で Shift+Enter 改行対応（#26）。CLI/MCP scrollback（計 51 ツール）
- 関連コミット: `8673993`（feature/42-remote-frontend-v2 worktree → PR #45 squash merge）
- 次: スマホ実機での最終確認（WS・リサイズ・履歴・Quick keys）

## 2026-07-03（Issue #54: TCC 許可ダイアログ頻発の根治 — 署名 DR の identifier 固定）
- 根因を実測で特定: ダイアログ = macOS 26 の kTCCServiceSystemPolicyAppData（per-target 許可。
  トリガーは claude sandbox の iCloud/Google Drive アクセス、tako 名義は responsible process のため）+
  署名 DR が証明書依存で不安定（Apple Development 2 枚 + head -1 選択）だと許可が全リセット。
  build-app.sh の codesign に explicit DR（identifier 固定）+ identity 選択の決定論化 + 署名検証を追加
- 関連コミット: `fix/54-tcc-signing` worktree → PR squash merge
- 次: 実機 TCC 検証（tccutil reset はユーザー同意が必要 → manual-checks.md 参照）

## 2026-07-03（Issue #60: リリースビルドに PWA ビルド工程を組み込み → v0.2.7）
- build-app.sh に npm ci + npm run build を追加（cargo build の前に実行）、release.sh に
  dist 鮮度の機械検証（JS に「履歴」マーカーが含まれるか）を追加。PR #61 squash merge
- 関連コミット: `2b9f85a`（#61）、`20a0bd3`（v0.2.7 リリース）
- v0.2.7 パッチリリース完了（GitHub Release + homebrew cask 更新）

## 2026-07-05（Issue #63: リモート UI 再設計 v3 — PC 非破壊・連続スクロール・リーダービュー化）
- WS の cols/rows 自動リサイズ全廃（PC 非破壊）+ init/update プッシュプロトコル刷新
  （`#{history_size}` 増分で押し出し行を切り出し）。xterm.js 廃止 → 折り返しリーダービュー
  （1 本の連続スクロール、下端追従/過去閲覧/追従再開）+ 自前 ANSI SGR パーサ。
  隔離 tmux で 93x50 不変の機械検証 + Playwright モバイル操作記録 + #51/#52 維持を PR に記録
- 関連コミット: `d99db5d`（feature/63-remote-ui-v3 worktree → PR #69 squash merge）
- 次: スマホ実機での最終確認（タッチスクロール・ソフトキーボード）

## 2026-07-05（Issue #64: 日本語混在行の半角文字消失を根治）
- 根因を実測で確定: 半角グループ div の幅を GPUI が wrap_width として扱い、シェイプ幅の
  f32 ヘアライン超過で末尾単語/文字が折り返されて overflow_hidden 外へ消失（純 ASCII でも発生、
  「max」丸ごと/「I」単体消失の観測と一致）。行 div whitespace_nowrap + セル幅不一致グリフ
  （⏺ 等）の個別 div 隔離で根治。#39 の要素数削減は維持、zed の force_width 方式で裏取り
- 関連コミット: `9ec7cd2`（fix/64 worktree → PR #70 squash merge）。セルフテスト 69b + unit 5 本追加
- 次: なし（#64 クローズ。見た目の最終確認は通常利用で）

## 2026-07-05（v0.2.8 リリース）
- #63/#64/#67/#59 の 4 件を v0.2.8 としてリリース。release.sh の PWA 鮮度マーカーを
  「履歴」→「ペイン」に修正（#63 でコメント化され minify 後消失）。CHANGELOG に [0.2.7]
  セクション新設（#60 入れ忘れ回収）。homebrew-tako cask 0.2.8 更新済み
- 関連コミット: `4abad45` `61fca52`、tag `v0.2.8`
- 次: スマホ実機テスト（#63 リーダービュー）+ PC で #64 半角消失の目視確認

## 2026-07-06（docs サイト内容刷新 → PR #73）
- セットアップページを tako setup 対応で全面刷新、CLI リファレンスを全コマンド詳細版に
  （shelve→background 等の実装乖離を多数修正）、MCP ツール一覧を実 51 ツールへ更新。
  リリースノート `/releases/` とオーケストレーション紹介 `/features/orchestration/` を新設
- 関連: ブランチ `docs/refresh-setup-releases`（3 コミット）→ PR #73（公開サイトのため main 直 push 回避）
- 次: PR #73 のレビュー・マージ（ユーザー判断）

## 2026-07-06（docs オーケストレーション解説をユーザー視点に刷新）
- orchestrator.md を「tako master 実践ガイド」に全面改稿（会話例・つまずき中心、設定ファイルは
  上級者向け参考に格下げ）、orchestration.md 簡潔化、getting-started に setup 設定項目一覧、
  quickstart.md 新設。「設定は master に頼むだけ」の世界観へ統一
- 関連: ブランチ `docs/orchestration-user-first` → PR squash merge（マージで自動デプロイ）

## 2026-07-06（公開前セキュリティ・公開可否監査）
- 全ブランチ 370 コミット + 現行を gitleaks / cargo audit / パターン grep で監査。秘匿情報ゼロ、
  判定「条件付き OK」（GPL-3.0-or-later のまま公開が必須条件。Apache-2.0 化は GPL 依存で不可）。
  レポート `reviews/2026-07-06_公開前セキュリティ監査.md` を main へ直接コミット（e37e585）、発見 7 件を Issue #75〜#81 に起票
- 次: #75 方針確定 → #76/#77 削除 → #81 履歴の扱い決定 → public 化

## 2026-07-06（コードベース全体レビュー: 構造・MCP リファクタ提案）
- 全 4 クレート精読 + clippy/test 全緑確認。提案 17 件（高5/中7/低5）を
  `reviews/2026-07-06_構造・MCPリファクタ提案.md` にまとめ main へ直接コミット。
  実バグ 1 件発見（orchestrator_run の output 常時空 = #82）。高優先度は #82〜#86 に起票
- 関連コミット: `a3ddd94` `[ドキュメント] コードベース全体レビュー`
- 次: #82（バグ）と #83（重複統合）の着手判断はユーザーと相談

## 2026-07-06（監査対応: 個人情報アーティファクトの削除 #76/#77 + 履歴受容決定 #81）
- `.playwright-mcp/`（20 点）と `.wrangler/cache/` を index から削除（ローカル保持）、
  `pwa-*.png` 2 点を完全削除、.gitignore に `.playwright-mcp/` `.wrangler/` を追加
- 履歴上の個人情報は「受容」で決定（#81 close。実名は author 名で公開前提・SHA 保持を優先）

## 2026-07-06（#82 + #83: orchestrator run の output バグ修正 + 完了待ちポーリング一本化）
- #82（Read 応答の text を content で参照し output 常時空）を修正、#83 で MCP / CLI に
  二重実装だった完了待ちポーリングを `tako-control::orchestrator::wait` へ一本化（単体テスト 9 本追加）。
  CLI のみだった tako 再起動時の gone 誤検知防止が MCP 版にも入り挙動統一。全テスト 351 緑
- 関連: PR #87 squash merge（`25ed398`）→ `build-app.sh --install` で実機反映
- 次: 実機で tako_orchestrator_run の output 動作確認（ユーザー）。レビュー残 Issue は #84〜#86

## 2026-07-06（リモート接続バグ調査: cloudflared 未導入時の無音 LAN フォールバック → Issue #89）
- 友人環境の「接続リンクがプライベート IP でページが開けない」を調査（コード変更なし）。
  根因 = cloudflared 不在時の LAN-only フォールバック警告が spawn_daemon の未読 stderr に消え、
  無警告で `http://10.x.x.x:7749` の URL/QR を提示（AP isolation 下で到達不能）。#89 起票 + #78 相互リンク
- 次: 修正はリレー worker 並行作業の完了と #78 認証方針の決定後（修正方針 5 点は #89 に記載）

## 2026-07-06（#88: tako setup に依存ツールチェック段階を追加）
- 実地報告（tmux 無しで `tako remote start` 失敗）を受け、setup 冒頭で claude（必須）/
  tmux・cloudflared・git（任意）を検出し、用途説明 + brew 案内 + その場インストール（y/N）を追加。
  cloudflared は #89 を受けて対象化。`--check` にも同一覧を表示。docs の依存表も同期
- 関連: PR #92 squash merge → `build-app.sh --install` で実機反映（3 経路の実バイナリ検証済み）

## 2026-07-06（公開前条件の解消: #75 ライセンス統一 + #78 リレー認証 → 判定 OK）
- #75: GPL-3.0-or-later へ完全統一（欠けていた 6 マニフェストに license 追加。PR #90）。
  #78: リレー登録を端末シークレットで first-write-wins 保護 + 公共インスタンス明記 + worker テスト（PR #93）。
  監査レポートの判定を「条件付き OK」→「OK」へ更新。残 OPEN は #79/#80（公開ブロッカーではない）
- 次: public 化可能。本番 worker デプロイ（npm run deploy・wrangler 認証要）はユーザー作業

## 2026-07-06（#78 本番デプロイ + #80 Trash argv 化 — 監査由来タスク完了）
- #78 リレー worker を本番デプロイ（version 5acac8f5）、稼働中インスタンスで別 secret/secret 無しの
  上書きが 403 になることを実地検証。#80 FileOp::Trash を argv 渡し化し AppleScript インジェクションを
  構造排除（PR #96、決定的テスト + e2e #[ignore]）。#79 は cargo update 不可を確認しコメントのみ
- 関連コミット: `d24cf55`（#96）。#79 は GPUI 依存更新待ち・macOS/Win 非配布のため OPEN 継続

## 2026-07-06（#94: tako setup にアップデート追従機能を追加）
- setup changelog（`resources/setup/changes.yaml`、revision 連番 + kind auto/guided）をバイナリ同梱し、
  config.yaml の `setup.applied_revision` と突き合わせて未適用変更を検出・対話追従。
  `tako-control::setup` 新設（config スキーマ移動 + CLI/MCP 共有）、CLI `tako setup --changes [--json]` +
  MCP `tako_setup_changes`（52 ツール）+ pending-changes.md + system prompt 追従フロー
- 検証: build / clippy(-D warnings) / fmt / test 全緑（362+）、セルフテスト既知 PDF 以外緑、CLI 実機 3 経路確認
- 次: PR squash merge → `build-app.sh --install` で実機反映

## 2026-07-06（#91: リモート接続の入口を tako-remote.pages.dev 固定 URL に一本化）
- トンネル + リレー登録成功時の connect_url を Pages 固定 URL（machine パラメータ付き）へ切替。
  トンネル直 URL は fallback_url で併記、LAN-only 落ちは CLI が明示警告（#89 の可視化に部分対応）、
  status 用に tunnel state 永続化、PWA は pages.dev 配信時の自己 health スキップ + version 互換警告。
  `scripts/deploy-pages.sh` 新設（release.sh --publish に組込み）で Pages へ実デプロイ済み
- e2e: 実トンネル + 本番リレー + Pages PWA（別オリジン）で resolve → 接続 → ペイン一覧まで全 200 確認
- 関連: PR #99 squash merge。リリースは master 側で別途

## 2026-07-06（v0.3.0 リリース）
- 今日の全変更（#88/#94/#78/#80/#82/#83/#75/#91）を v0.3.0 としてリリース。CHANGELOG に
  #88 エントリを回収し Unreleased を [0.3.0] へ、release.sh に --generate-notes 併用を追加。
  annotated tag `v0.3.0` + GitHub Release（zip 添付）+ cask 0.3.0 + Pages デプロイ +
  /Applications へ v0.3.0 配置済み
- 関連コミット: `4886300`（tako）/ `5aaf98a`（homebrew-tako）、tag `v0.3.0`
- 反映確認済み（2026-07-06 21:05 再起動）: 実行中アプリ 0.3.0 / MCP 52 ツール（tako_setup_changes 含む）/
  リモート固定 URL のリンク継続・setup --check の新依存チェックも実機確認済み

## 2026-07-07（#95: claude TUI の Enter 空振りを修正）
- 実機 transcript + 実 claude 実験で根因を確定（LF=改行挿入 / Enter 代行の検証欠陥 /
  busy 中の CR 取りこぼし）。人間 Enter の送達検証 + 自動再送、Enter 単独送達フロー
  （dispatch + deliver_via_tmux）、直接 write の LF→CR 正規化を実装
- 検証: build / clippy(-D warnings) / fmt / test 全緑（unit +3）、実 claude e2e 2 本
  （Enter 単独送達 新規 + 事前信頼送達 回帰）緑
- 次: PR squash merge → `build-app.sh --install` → tako 再起動（ユーザー）で GUI 経路の実機確認

## 2026-07-07（#100: オーケストレーション品質パイプラインの標準化）
- master 用 default system prompt に task-intake（依頼列挙 → 1 worker = 1 成果物）/
  worker-prompt-template（受け入れ条件・検証手順・証拠つき報告の型）/ acceptance
  （証拠と diff で検収してから報告）を新設。setup 配布物に CLAUDE.md セクション
  06-completion-verification 新設 + changes.yaml rev 5（guided）で既存ユーザー追従
- 設計意図は `reviews/2026-07-07_オーケストレーション品質設計.md`。docs 2 ページ更新
- ローカル反映済み: master-system.md → .bak-20260707 退避、個人ルールは local-rules.md +
  profiles の prompt_blocks.append へ移行。`build-app.sh --install` 済み（反映は tako 再起動後）
- 次: tako 再起動後に `tako master` で分担計画・検収挙動を実運用確認

## 2026-07-07（v0.3.1 リリース + connect_url トークンマスク修正）
- 追加バグ修正（#104）: `remote status` の既定マスクで token フィールドは *** だが
  connect_url/fallback_url のクエリに token=生値が残っていた → `mask_token_in_url` 新設で
  URL 内 token= も伏せる（--show-token/MCP show_token=true で生値）。単体テスト 2 本追加（PR #106）
- v0.3.1 リリース: version bump + CHANGELOG [0.3.1]（#104 を Security 記載、#95/#100 同梱）。
  annotated tag `v0.3.1` + `release.sh --publish --skip-build`（zip + Pages デプロイ +
  gh release --generate-notes）+ `build-app.sh --install`（/Applications 0.3.1）
- トークンローテーション実施: remote stop→start（旧 pid 10485 の leaked token を無効化 →
  新 pid 19941・新トンネル・token マスク確認）。secure start が実トンネルを張って成立まで観測
- 関連コミット: `1636683`（#106）、tag `v0.3.1`、Release https://github.com/takushio2525/tako/releases/tag/v0.3.1
- 次: リレー worker のレートリミットは live relay 未反映（`cd web/tako-remote-worker && npm run deploy` が別途必要 = ユーザー作業）

## 2026-07-07（#104: tako remote セキュリティ監査 + 推奨対応6件実装）
- 再監査レポート `reviews/2026-07-07_takoremote再監査.md`（認証/暗号化/外部依存/漏えい/
  任意コマンド実行 + 日本法リスク整理）を作成。推奨6件を実装: ①暗号化トンネル必須化
  （張れなければ起動拒否、平文は --insecure で明示 opt-in）②token/QR を 0o600 ③status の
  トークン既定マスク（--show-token / MCP show_token）④トークン比較の定数時間化 ⑤リレー
  worker のレートリミット（IP 単位）⑥README/docs 注意追記
- 検証: build/clippy(-D)/fmt/test 全緑、worker npm test 7/7、insecure serve を実バイナリで
  e2e 観測（平文警告・LAN 直 URL・token 0o600・status マスク）。secure 拒否は cloudflared を
  隠せず runtime 未観測（コード+build で担保、レポートに明記）
- 関連: PR #105 squash merge（`5782367`）→ `build-app.sh --install` で 0.3.0 実機反映済み
- 次: tako 再起動で GUI 経路の実機確認（`remote start` が新 CLI で --insecure/拒否を反映）

## 2026-07-07（#95 実機検証完了 + #103 起票）
- tako 再起動（14:08、新プロセス確認）後に #95 修正を実機検証: プローブのバイト観測で
  Enter 代行が「括りなし CR 即発火」（旧: 空括り+13 秒）、残留テキストの Enter 代行
  4 連続成功、busy（生成）中の Enter 送達が queue 成立 → タスク完了後の自動送達まで確認
- 副産物: Cmd-Q で tako が終了しない事象（2 回再現、Dock 終了は正常）を #103 に起票（未修正）
- 次: なし（#95 クローズ済み。次リリースで Unreleased の #95/#100 を出荷）

## 2026-07-07（#107: read_pane でゴーストテキスト/手動入力の判別機能を追加）
- screen.rs の StyleRun/CellStyle に dim フラグ追加、analyze_input_line() で ❯ 行の
  dim 状態を分析し ghost/user/mixed/none を判定。dispatch の Read 応答に input_status
  フィールド追加（MCP + CLI 両対応）。テスト 6 本追加、全 115 テスト緑
- 関連コミット: `2ac8ce9`（PR #108 squash merge）、build-app.sh --install 済み
- 次: tako 再起動で実ペインでの input_status 実機確認

## 2026-07-07（#109: 複数 master 並行時の spawn 混線を修正 → v0.3.2 リリース）
- MCP セッションに `caller_role`（`TAKO_ORCHESTRATOR_ROLE` 由来）を追加。`caller_pane` が
  stale で `resolve_pane` 失敗時、role suffix で正しい master を特定するフォールバック実装。
  回帰テスト 3 本追加。v0.3.2 リリース（tag + Release + Pages + /Applications 配置）。
  リレー worker レートリミットも本番反映（register/resolve 正常系確認済み）
- 関連コミット: `b3ed19d`（PR #110）、`665d541`（v0.3.2）、tag `v0.3.2`
- 次: tako 再起動で新バイナリ反映
## 2026-07-08（#113: 多重起動によるペイン消失を根治 + フリーズ診断導入）
- 根因 = 多重インスタンスの並行復元（`-A -D` クライアント強奪 → Exited 途中状態が layout.json を
  上書き → 次回起動の orphan cleanup が実行中 worker を kill する三段連鎖）。修正 = 多重ガード
  （セカンダリモード FR-5.8）+ cleanup の activity 1h 猶予 + 二重発火冪等化 + perf.log 診断
  （UI ストール / dispatch 遅延）+ window capture の background 化。隔離環境で修正前後を実演
- 関連: PR #114 squash merge（`fe73b60`）。副産物 #115（GitLog 2431ms UI 専有）/ #116（テストソケット残骸）
- 実機確認済み → #113 close: 再起動復元に回帰なし / 2 個目起動でセカンダリモード（persist.log
  「復元スキップ」）/ プロンプト無し worker 20 匹スポーンで tako 74MB・CPU16%・ペイン消失ゼロ
  （UI ストールは 0.83s が 1 回のみ）。フリーズ恒久根因は perf.log で追跡継続

## 2026-07-08（#111: tako solo コマンド実装完了 → merge）
- 前任 WIP を仕上げ。mod.rs 側（solo ロジック + テスト）は完成済みだったが CLI に solo が
  無く、別機能 sessions の未定義型断片が混入しビルド不能だった。solo CLI（`orchestrator_solo`、
  master 対称・`build_master_claude_cmd` 共用・role/env `solo`/`solo:<suffix>`・effort=high・
  solo-profiles/ 分離）を新規実装。sessions 断片は除去（無関係・保全コミット `9783c33` に保存）、
  tako-app ツール数を 52 へ戻す。実バイナリで構築コマンド/role/effort/prompt 注入 + エッジ 2 件を検証
- 関連コミット: `9783c33`（WIP 保全）、`99a1f4c`（solo 実装）→ PR #117 squash merge（`53bdf1b`）
- 実機確認済み → #111 close: `tako solo` でタブ 'solo' 起動・effort=high 実測（`· H`）・
  solo prompt の 3 本柱（エコ運用 / spawn 禁止 / projects 把握）を確認。実対話の細部は通常利用で

## 2026-07-10（コードベース全体 / tako remote 再レビュー）
- daemon・Cloudflare relay / Pages PWA・REST/WS・tmux を横断監査。remote は P0 対応前提、全体本体は層分離・テスト文化を高評価。コード変更なし
- 検証: Worker 7/7・PWA build・npm audit 緑
- 全760行の詳細レポート: `reviews/2026-07-10_gpt5.6solレビュー.md`（実施日時・対象 commit・全所見・対応ロードマップを収録）
- 接続方式の設計検討を `reviews/2026-07-10_tako-remote接続方式・認証設計.md` に保存（Cloudflare Access + tako機器認証、Tailscale、SSH、専用クラウドを比較）

## 2026-07-10（#118: FDA ガイド機能の実装）
- macOS TCC の毎回フォルダアクセス許可ダイアログ対策。`tako-control::fda` 新設（FDA 状態検出 +
  システム設定オープン）+ dispatch `Fda` + MCP `tako_fda`（計 53 ツール）+ CLI `tako fda status/open`
  + `tako setup --check` に FDA チェック追加。build / clippy / fmt / test 全緑（117 passed）
- 次: PR squash merge → `build-app.sh --install` → 実機検証

## 2026-07-10（#120: worker の codex / agy 対応 → merge + 実機反映）
- worker のエージェント CLI を claude / codex / agy から選択可能に。`orchestrator::agent` 新設 +
  TUI 検出の和集合化 + Profile `worker_agent`/`worker_agents` + spawn/run/profiles の agent 系を
  MCP・CLI に 1:1 公開。agy フッター「(Thinking)」への busy 誤爆（永遠に完了しない）を実機検証で発見・修正
- 関連: PR #122 squash merge（`f8a8b3c`）。全緑（429 tests）+ セカンダリモード併走で
  codex / agy / claude 3 種の spawn → 完遂 → send_input → WORKER_IDLE を実機検証済み
- 次: tako 再起動で新バイナリ反映（agy worker は profiles set --agent agy --agent-skip-permissions true 推奨）

## 2026-07-11（#124: PDF プレビューのテキスト選択・クリップボードコピー）
- PDFKit FFI でテキストレイヤ抽出 → 既存 preview_line_bounds/texts に統合。ドラッグ選択・
  ⌘C コピー・ハイライト描画が Code/Markdown と同パス。テキストなし PDF 防御 + テスト 2 本
- 関連: PR #125 squash merge（`ba0bc7a`）。build / clippy / fmt / test 全緑（354 passed）
- 次: ユーザーによる GUI テキスト選択の実機確認（マウスドラッグ→⌘C→pbpaste）

## 2026-07-11（#127: master の codex 対応 → merge + 実機反映）
- プロファイル `master_agent`（claude / codex）で master / solo のエージェント CLI を選択可能に。
  codex は developer_instructions で system prompt 注入 + `-c mcp_servers.tako.*` 一時注入
  （env_vars で TAKO_* 引き継ぎ）。波及ガード（master≠claude の model/effort を claude worker へ
  非継承）+ agy は master 非対応の明示エラー。CLI `--master-agent` / MCP master_agent で 1:1
- 関連: PR #128 squash merge（`954330c`）。全緑（437 tests）+ 実 e2e（codex master 起動 →
  /mcp で tako 全 53 ツール列挙）+ エッジ 3 種（gemini / agy master / agy solo が起動前エラー）
- 次: tako 再起動で新バイナリ反映 → sol プロファイル作成（ユーザー）。codex への実プロンプト
  送信検証は利用上限解除（7/11 20:40）後

## 2026-07-12（コードプレビュー軽量編集 #126）
- FR-3.5: UTF-8 安全なその場編集、dirty / ⌘S、外部変更拒否を実装。dispatch 3 操作 + `tako edit` + MCP 3 ツールで AI 操作も同期
- 検証: workspace build / test（446 pass）/ fmt / clippy 緑。PDF #124 テストも緑。セルフテストは既知の CoreGraphics PDF 項目70のみ失敗

## 2026-07-12（#132: codex/agy 承認既定スキップ + profiles set --worker-model-policy + target 掃除）
- codex/agy worker は既定 skip_permissions=true、codex master は --dangerously-bypass-approvals-and-sandbox でMCPツール承認もバイパス。
  CLI `--worker-model-policy` + MCP `worker_model_policy` 追加。`scripts/clean-target.sh` 新設
- 検証: 450 tests / fmt / clippy 緑。codex exec --dangerously で MCP 呼び出し承認バイパス実証。profiles set --worker-model-policy delegate → YAML 反映確認
- 関連コミット: PR #133 squash merge（`b9b5b33`）+ `3739385`（-a never→bypass修正）

## 2026-07-12（#134: ファイルツリーへの AI フォルダ追加・削除）
- `tako tree add/remove/list` + MCP `tako_tree_folder`（計 57 ツール）。タブ単位・layout.json 永続化。
  Tab に pinned_folders を追加し、sync_filetree_roots で cwd 由来 roots と合流表示。
  master/solo system prompt にフォルダ追加ガイド追記
- 検証: テスト 5 本 + build / fmt / clippy 全緑。実機は tako 再起動後に確認
- 関連コミット: PR #135 squash merge（`cd57d77`）

## 2026-07-12（#136: エージェント共通ルール同期機能の追加）
- `tako agents sync-rules` / `tako agents status` + MCP `tako_agents_sync_rules`（計 58 ツール）。
  正本ファイルの内容を各エージェント指示ファイルにマーカーブロックで埋め込む。ブロック外不変・バックアップ付き
- 検証: テスト 5 本 + build / fmt / clippy 全緑。一時 HOME で初回/再同期/unchanged/マーカー壊れ/正本空のエッジケース全通過
- 関連コミット: PR #137 squash merge（`744c3c5`）

## 2026-07-12（#141: ファイルツリー追加をプロンプトで積極指示）
- master / solo 両方のデフォルト system prompt behavior 項目 6 を強化。会話中のプロジェクト・関連フォルダを聞かれる前に追加する行動規範に
- 関連コミット: PR #142 squash merge（`8bb2104`）。build-app.sh --install 済み

## 2026-07-12（#143: setup の FDA 案内ステップ強化）
- TCC ダイアログ頻発の原因説明・設定画面を開く対話・再起動案内を追加。changes.yaml rev 6 で既存ユーザーにも配信
- 検証: 460 tests / fmt / clippy 全緑。実機 `setup --check` で付与済みパス確認、`--changes` で rev 6 配信確認
- 関連コミット: PR #144 squash merge（`f97ca1a`）。build-app.sh --install 済み

## 2026-07-13（#146 + #147: cmd+クリックリンク機能）
- URL（#146）とファイル/ディレクトリパス（#147）の cmd+ホバー下線 + cmd+クリック開くを実装。
  links.rs を tako-core に新設（GPUI 非依存）。URL テスト 12 本 + パステスト 10 本。
  パス解決は cwd 相対 / ~ 展開 / 絶対パスの 3 戦略 + 実在チェック。:行:列 サフィックス除去対応
- 関連コミット: PR #148（`c4af877`、#146）+ PR #149（`42a7322`、#147）。build-app.sh --install 済み
- 次: tako 再起動で実機確認

## 2026-07-13（#145: プレビュー選択座標 / PDF / 編集色）
- GPUI 実 shaping + 最近傍 UTF-8 キャレット、PDFKit 文字矩形、編集時 syntect 色を統合。selftest 40 の固定待ちと 66b-2 の二重 update、既存 PDF fixture も修正し全セルフテスト完走
- 関連: PR #151 squash merge（`c5618ca`）+ install 済み。#150 は 3 件とも selftest panic と確認して close

## 2026-07-13（#152: PDF 選択実描画 / 標準言語シンタックス色）
- PDF canvas の画像下端 static position を根治し、syntect の改行保持 + 全標準言語共通解決を実装。Metal RGBA で PDF / C++ / Python の実ピクセル変化を確認
- 関連: PR #154 squash merge（`6f7cd1c`）+ `/Applications/tako.app` install・署名検証済み

## 2026-07-13（#153: パスリンク cmd+クリック不動作の根治 + cmd 押下中の下線・ハイライト）
- 根本原因 5 件を修正: ①cell_at のクランプでリンクホバーが最初のペインへ誤ヒット ②ディレクトリ
  クリックが pending_attach 後処理欠落で空ペイン ③TUI（OSC 7 なし）で cwd 不明 → 起動時 cwd を
  セッション初期値に ④cwd=None でパス検出ごとスキップ ⑤リンク走査の無限ループ。装飾は下線 +
  accent + 背景をリンク文字列だけに限定、cmd 単独押下でも即時更新。選択ドラッグは cell_at_clamped
  分離で旧挙動維持（引き継ぎ検証で発見・修正）
- 検証: 隔離セルフテスト完走（69c 全 7 判定パス）+ build / test / fmt / clippy 全緑
- 次: tako 再起動 → manual-checks.md #153 節の GUI 確認

## 2026-07-13（#155: Web ビューを wry (WKWebView) ネイティブ統合へ全面刷新）
- CDP ミラー PoC（座標ずれ・クリックのみ・Chrome 依存）を wry `build_as_child` へ置換。
  直接操作（クリック/スクロール/IME = OS 配送）+ dock 退避/復帰（ページ生存）+ 永続化 +
  ポート検知チップ統合。dispatch `Web` / CLI `tako web` / MCP `tako_web`（9 action、58 ツール不変）
- タイトル追跡は ipc 不達（data: URL、実機診断で確定）のため eval 2 秒ポーリングへ。
  検証: 487 tests / fmt / clippy 緑 + セルフテスト完走（項目 71 = webview e2e 8 操作）
- 関連: PR #160 squash merge（`6705c39`）+ #163（CLI 基準ペイン任意化、実機検証で発見）+
  install 済み。実機 e2e（セカンダリ + CLI: open → read title=Example Domain → close）+
  screencapture ピクセル確認済み
- 次: tako 再起動 → manual-checks「Web ビューペイン」節の GUI 確認

## 2026-07-13（#103: Cmd-Q 不発の根治 — Quit のグローバルアクション化）
- 根因を GPUI ソースで確定: Quit がルート div の on_action のみでフォーカスパス依存。blur（focus=None）時は
  dispatch path が root node へフォールバックしキーバインド・メニュー両経路とも不発（Dock 終了のみ AppKit 経路で生存）。
  修正 = `cx.on_action` グローバル化 + 終了処理を `cx.on_app_quit` へ（Dock/OS 終了でも layout 保存。quitting ガードで #30/#113 維持）
- 検証: 同一セルフテスト（blur + cmd-q）が旧構造 FAILED → 新構造 OK / 実 Cmd-Q キーイベントで隔離インスタンス終了 /
  exit 全ペイン終了経路の回帰なし / 486 tests + fmt + clippy 全緑

## 2026-07-13（v0.4.0 正規リリース + 夜間リリースのローカル launchd 化 #166）
- v0.4.0 リリース: CHANGELOG に v0.3.2 以降の未記載 13 件（#113/#118/#120/#124/#127/#129/
  #132/#134/#136/#141/#143/#146-147+#153/#103）を英日併記で回収 → tag `v0.4.0` +
  バイナリ付き GitHub Release + Pages デプロイ + homebrew-tako cask 0.4.0（`c18dcae`）
- 夜間リリースを scripts/nightly-release.sh（launchd 毎日 5:00）へ移行。クラウドルーチンの
  三重苦（バージョン計算・main 直 push・macOS バイナリ不能）を解消。スキップ 3 経路 +
  dry-run bump 判定を実機検証、bash 3.2 の変数名境界バグも修正
- 関連: `98b17ea`（リリース）/ PR #170 squash merge（`1c2c48a`）、Issue #166 クローズ
- 次: 明朝 5:00 の初回 launchd 実行で v0.4.1 自動リリースの通し検証

## 2026-07-13（#169: projects.yaml 並行 add 全消失の根治 — config_io 新設）
- 根本原因を実証テストで確定: ①旧 save = fs::write の truncate→write 窓 ②serde_yaml が
  空 / 部分 YAML を「0 件」で成功パース ③RMW のプロセス間直列化なし、の三段連鎖。
  新設 `config_io`（アトミック書き込み + `<path>.lock` flock + .bak.1〜3 世代バックアップ）へ
  projects.yaml / profiles/*.yaml / config.yaml の書き込みを集約、mutate 系 API で fail-loud 化
- 検証: 507 tests / fmt / clippy 全緑 + 実機 before/after（修正前 = 並行 add 60 件で 48 件消失、
  修正後 = 118/118 全件残存・破損 YAML 拒否・bak 復元成功。隔離 HOME）

## 2026-07-13（#159: ターミナルスクロールの大幅改善 — ピクセル単位化・ミラー方式・スクロールバー）
- Zed エディタの行小数 scroll_position 方式をターミナルへ翻案: 直接ペインは
  display_offset - fract 分解 + サブライン描画（visual-test 実ピクセル実証 direct=22197/shifted=0）。
  バックエンド(tmux)ペインは copy-mode 駆動を廃止し capture ベースのローカル履歴ミラーへ
  （tako-core::scroll_mirror 新設。行単位・往復レイテンシ・キー飲まれを構造解消）。
  スクロールバーはホバー維持 + サム強調。CLI/MCP Scroll は ControlHost::backend_scroll_view で同一経路
- 検証: 全テスト・隔離セルフテスト（44b/61b-61e 新設・更新）・visual-test 全緑
- 次: merge + install 後に manual-checks.md「ターミナルスクロールの大幅改善」節の人手確認

## 2026-07-13（#165: worker spawn のレイアウトエンジン）
- spawn を master-reserved（master の取り分維持 + 右側 worker 領域の grid/spiral 配置）へ刷新。
  worker 領域は spawned_by チェーン判定（ユーザーペイン不変）、close 時は領域内のみリフロー。
  config.yaml spawn_layout + `tako orchestrator layout` + MCP `tako_orchestrator_layout`（59 ツール）。
  master/solo プロンプトにレイアウト行動規範を追記
- 検証: tako-core 単体 10 本 + セルフテスト項目 72 + セカンダリ実機 spawn ×4 → 十字四分割 →
  close リフローの screencapture ピクセル確認。全テスト / fmt / clippy(-D warnings) 緑
- 副産物 #178: TAKO_DISCOVERY_DIR 指定で多重起動ガードが無効化され production の tmux
  バックエンドを強奪する穴を発見・起票（実プロセス損失ゼロ、ユーザー復旧済み）

## 2026-07-13（#177: 全ターミナルペイン消失の根治 — 復元強奪ガード + 縮退保存ガード + tako recover）
- 根本原因を worker トランスクリプト + persist.log[pid] + perf.log で特定: TAKO_DISCOVERY_DIR だけ
  隔離した dev 検証起動が多重ガード（control.json のみ参照）を素通り → 本番 layout 復元 →
  `-A -D` が本番 GUI のクライアント 13 本を強奪 → PTY 一斉死亡 → 縮退 layout 上書き（16:53 の
  「再起動」は実在せず。クラッシュレポート無し・本番プロセスは 16:57 の kill -9 まで生存）
- 三層防御: 復元強奪ガード（FR-5.10。list-clients + 祖先辿りでセカンダリ降格）/ 縮退保存ガード
  （FR-5.11。半減保存前に .bak.1〜3 退避 + 10 分回転ガード）/ TAKO_ISOLATED=1 一括隔離。
  + `tako recover`（一覧 / --apply / --force）+ persist.log 行に pid + README 復旧手順
- 検証: 全緑 + 隔離セルフテスト完走 + 実機 e2e（強奪防止・事故再現・bak 退避・recover 復旧の通し）
- 次: PR squash merge → install → Issue クローズ

## 2026-07-13（#167: マウスエスケープ断片の入力欄混入を根治 — send-keys 直接注入 + レート制限）
- 機序を隔離 tmux + 実 claude で実測確定: SGR シーケンスが途中で 10ms+ 途切れる（洪水の
  部分 write + UI 停滞）と tmux（escape-time 10ms）が ESC を単独確定し残りを平文転送。
  `\x1b[<6` + 600ms + `4;45;18M` で観測断片と完全一致の混入を再現。仮説①②の単純形は棄却
- 二層対策: バックエンドのホイールレポートを `send-keys -H` 直接注入へ（外側 PTY 非経由 =
  構造的根絶。`scroll_mirror::send_wheel` + `pump_wheel` 直列化 + `#{mouse_sgr_flag}` 出し分け）
  + 全転送にトークンバケット 150 ev/s・バースト 8（`terminal.rs`）
- 検証: 551 tests / fmt / clippy 全緑 + 隔離セルフテスト完走 + 実 claude before/after
  （before = 入力欄へ断片大量混入、after = idle 1500 + busy 588 イベントで断片ゼロ）
- 次: PR squash merge → install。並行 #181 へ変更点を Issue コメントで共有済み

## 2026-07-13（#181: スクロール改善が実機で体感できない問題の根治 — 3 根因 + カクつき）
- 根因 ①ミラー経路判定が backend_sessions のみで TmuxOpen ビューペインが直接ペイン扱い
  （alt screen = 履歴 0 で不発）②persist ON では外側 PTY も backend ラップされ backend 優先
  解決だと外側（history 0）へ誤解決 ③persist 復元後は tmux_view_panes 未登録 + ネスト候補が
  既定サーバーのみで `--socket tako` のビュー先を辿れない。カクつき = worker_status dispatch が
  claude CLI（実測 550〜1100ms）を UI スレッド同期実行（perf.log 2h で 2000 件超・報告時刻一致）
- 修正: mirror_scroll_pane / mirror_source（ビュー先優先）+ ネスト候補に backend socket 追加 +
  worker_status を snapshot（UI）/compute（background）分離 + scrollbar_overlay 極小領域 panic 防御
- 検証: 全 551 テスト + 隔離セルフテスト完走（項目 73/74 新設）+ visual-test（direct=22197/
  shifted=0 = #176 記録値一致）+ 隔離 e2e キャプチャ 3 種（backend / ビュー / 復元ビュー）+
  worker_status 15 連打中 scroll 24〜34ms 安定・perf.log 0 件
- 副産物: 調査 CLI の TAKO_SOCKET 注入による本番誤接続（ビューペイン 1 個生成 → close 復旧済み、
  Issue に記録）。alt screen TUI 内スクロール粒度はアプリ依存 = 仕様と明確化（manual-checks 記載）

## 2026-07-13（#168 + #115: パフォーマンス改善 — メインスレッド非ブロック化 + PDF 描画キャッシュ）
- perf.log 実測（本番 3.3h）で 3 犯確定: ①OrchestratorWorkerStatus dispatch が claude agents
  --json（Node 起動）+ tmux + ps を UI で同期実行（4124 回 avg687ms、UI ストール全件と共起）
  ②PDF 表示中の毎フレーム Image::from_bytes 全バイトハッシュ（71p で render p50 96ms）
  ③PDF/動画ロード同期実行（open 1354ms）。白: save_layout/flock・リンク走査・通常 render
- 三本柱: dispatch offload（prepare_offload/OffloadJob。worker_status/git log/diff を
  background 化 + claude agents TTL 2s キャッシュ）/ PreviewImageCache（Arc<gpui::Image>
  再利用）/ 重量プレビューの background ロード（Loading → 差し替え）。恒久診断 perf_span +
  watchdog + TAKO_PERF_VERBOSE/TAKO_PERF_LOG 追加
- 実測: 並行 list 159〜204ms → 4〜5ms / PDF render p50 96ms → 1〜3ms / open 1354ms → 48ms
- 検証: 553 tests / fmt / clippy 全緑 + 隔離セルフテスト完走（PDF 3 項目は完了待ちポーリング化）。
  #181（worker_status snapshot/compute の先行修正）とは rebase 時に OffloadJob へ一本化
  （#181 のテストは検証内容を維持して新 API へ移植）
- 次: PR #187 squash merge → install → tako 再起動 → ユーザー体感の再確認依頼

## 2026-07-13（#157: orchestrator watch に異常検知イベント WORKER_ERROR を追加）
- watch がペイン画面から実採取パターン（API Error / usage limit / codex モデル切替ダイアログ）を
  検知し `WORKER_ERROR: tako:<pane> (<種別>)` + detail/action 行を出力。worker_status は
  status=error + error{kind, detail, recommended_action}（resume / wait_reset / respond_dialog）を
  MCP / CLI 1:1 公開、run は worker_error + auto_close スキップ。busy 中不判定・自動切替除外・
  末尾 15 行限定の誤検知ガード + master prompt にリカバリ手順（respawn 禁止）
- 検証: 581 tests / fmt / clippy 全緑 + 隔離 e2e（WORKER_ERROR 実測 35 秒・正常 idle 誤発火なし・
  close 時 WORKER_GONE 優先・MCP 直叩き一致・codex limit 画面で usage_limit 優先）
- 関連: PR #190 squash merge（`9847ee5`）→ install 済み。Issue #157 クローズ + 実測証拠コメント
## 2026-07-14（#112: セッション会話ログの管理と復元 — カタログ + ペイン平文ログ）
- A: セッションカタログ（FR-5.12。`tako-control::sessions` 新設）: 会話は claude transcript
  参照 + メタデータのみを sessions.yaml へ索引化。spawn 時 pending 記録（Issue 番号抽出）→
  claude セッション検出で昇格。`tako sessions list/show/resume` + MCP（resume は claude のみ）。
  B: ペイン平文ログ（FR-5.13。`tako-core::pane_log` 新設）: 確定行の増分保存
  （直接 = alacritty history / バックエンド = tmux capture）。TUI はマーカーのみ・
  5MB ローテ + 200MB 全体上限。`tako logs` + MCP（計 63 ツール）
- 副産物: spawn 応答 tmux_session の常時 null を修正（reserve_backend_session）、
  TAKO_DATA_DIR 隔離を新設（#177 の TAKO_PERSIST=1 併用穴を閉塞）
- 検証: 全緑（591+ tests）+ セルフテスト完走 + 隔離 e2e（spawn → 全滅 + 再起動 →
  resume → 文脈維持を実測）。ペイン kill 後の logs 読み出し・TUI 93B・洪水 26KB 実測
- 次: origin/main rebase → PR（Closes #112）→ squash merge → install

## 2026-07-14（#210: master identity — 復元後 role 消失 + 同一プロファイル複数 master 誤認の根治）
- orphan 復元で role 引き継ぎ（`TAKO_ORCHESTRATOR_ROLE` 逆引き）+ stale pane map（旧→新 pane ID）
  + self/spawn の caller 解決に stale map 挿入。テスト 9 本追加（645 全緑）
- 関連コミット: `0dbd534`（PR #215 squash merge）。`build-app.sh --install` 済み
- 次: tako 再起動で反映。手動 role 後付けは `tako title --pane <id> --role <role>`

## 2026-07-14（#212: 画面が重い・点滅・スクロールもっさりの根治 — pmset UI スレッド実行の排除）
- 犯人を perf.log + 隔離実測で確定: sleep guard（#173）の AC 判定 `pmset -g batt` が UI スレッドで
  2 秒毎に同期実行（アイドル 20〜30ms、CPU 飽和時に秒級）。IOKit FFI へ置換で
  periodic_prep p50 17〜59ms → 0ms / max 116ms → 8ms。サブスパン診断 + perf.log 行混線修正も同梱
- 外因も特定: worker 4 体の cargo build 並走で load avg 最大 161・swap 10.5/11GB・ディスク 99%
- 検証: build / fmt / clippy(-D warnings) / test 全緑（638 passed）+ FFI の AC 判定を pmset と実機突き合わせ

## 2026-07-14（#217: UI 大刷新 — Claude Design カンプの忠実再現 + 絵文字全廃）
- カンプ（design/claude-design/tako-ui、コミット済み）を正に M1〜M7 で全面刷新: テーマ基盤
  （ライト/ダーク = `tako theme` + MCP `tako_theme`、74 ツール）/ ピル型タブバー + ⌘K + ベル +
  テーマボタン（タイトルバー統合）/ ペインヘッダ（番号バッジ・workers ▾・↳ 親・cwd チップ・
  failed 赤 + 再実行）/ サイドバー（ブランチチップ・パスコピー・git サマリ）/ ステータスバー
  （breadcrumb・5h/週リミット・ctx 改良）/ 右パネル 3 タブ + orch ビュー + トースト + ⌘K パレット /
  絵文字全廃（tako-app grep 0 件、SVG アイコン 36 種を assets/icons/ui に新設）
- 検証: build / fmt / clippy(-D warnings) / test 全緑（988 tests）+ 隔離セルフテスト完走
  （33b テーマ MCP e2e・75 パレット新設）+ 隔離実機スクショでカンプ突き合わせ
- 次: PR（Closes #217）→ squash merge → install → Issue に証拠 + 目視チェックリスト

## 2026-07-14（#226: setup の claude / codex / agy 対応 + プラン別推奨）
- 3 CLI の検出・認証・プラン取得と対話フォールバック、単一自動選択 / 複数選択、プラン規模別 profile 推奨を実装。changes revision 8 と docs を同期
- 隔離 HOME / PATH で claude 単独・3 CLI から codex 選択を実測し、build / fmt / clippy / workspace test / docs build を全緑確認

## 2026-07-14（#231 / #234: PDF 品質改善 + PDF・画像ズーム）
- 行間ドラッグ全文選択を修正し、device scale × zoom × 表示幅の background 再ラスタライズを追加。Retina 全幅で 1224×1584 → 1920×2485、render p50 1ms を隔離実測
- PDF・画像の 25〜400% ズーム / パン / ページ維持リセット / 倍率表示を実装し、dispatch・CLI `tako preview`・MCP `tako_preview_view`（75 ツール）へ 1:1 公開
- workspace 全検証と隔離セルフテスト（PDF 150% raster key・文字 hit を含む）を完走。canvas 座標反映は effect cycle 末尾へ送り GPUI 再入更新を防止
- keyboard modality 直後も捕捉できる pinch 経路を追加し、隔離 E2E で 1.500 → 1.650 → 1.485 と全セルフテスト完走を確認

## 2026-07-15（#233: プレビューライブリロード）
- OS ネイティブ監視 + 300ms デバウンス + background 再生成を実装し、編集競合保護と CLI / MCP（全 80 ツール）を 1:1 公開
- 連続 6 write を 1 回・427ms で反映、状態保持、削除 / rename / 巨大ファイル / PNG / PDF、UI 専有 0ms 水準を隔離実測。全検証と全 diff レビューを完了

## 2026-07-15（#258: アプリ全体メモリ監査・調査マイルストーン）
- 6ページPDFの倍率世代で0.48GB→2.63GB、`MALLOC_LARGE` 1.30GB + graphics 1.08GBを実測。旧GPUI asset未除去が主因で、71ページ・同6世代は約27.35GiB相当
- ライブリロード8回でラスタライズ7本並行・RSS最大808,656KiB。BG退避/closeは解放なし、端末・sessions・logs・worker eventsはGB級原因でないと切り分け
- 次: 512MiB既定のバイト予算付きLRU + GPUI eviction、可視近傍デコード、reload single-flightを実装

## 2026-07-15（#258: メモリ上限・解放修正マイルストーン）
- 512MiB既定のバイト予算付きLRU、PDF可視近傍3ページ遅延デコード、GPUI CPU asset + GPU atlas明示解放、旧動画frame解放を実装
- ライブリロードをpane/path単位single-flight + 最新1件へ直列化し、未回収run履歴256件上限・pane補助cache close cleanupを追加
- dispatch / CLI `preview-cache` / MCPを1:1公開。app 91・CLI 25・control 425・core 276件の対象テスト全緑
