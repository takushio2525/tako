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

## 2026-07-15（#258: メモリ安定性検証マイルストーン）
- PDFズーム・ページ移動・ライブリロード30サイクルでfootprint peak 795MB横ばい、終了後RSS 84,816KiB、close後68,672KiB / LRU 0 bytesを実測
- #257統合後の追加21サイクルもpeak 812MB横ばい・RSS傾き負、render p95 / p99最大6ms。全品質ゲート + 隔離セルフテスト完走

## 2026-07-15（#258: 完了）
- PR #260を`530d568`へsquash mergeし、ブランチ削除・Issue完了コメントまで実施。installはmaster側へ引き渡し

## 2026-07-15（#262: setup UX 全面見直し — 根本原因調査）
- 実ユーザー設定の隔離コピーで v0.5.3 setup を 2 回実走し、両方とも CLI 側だけで
  5 問を再現。GPT 検出だけ採用、前回 agent / plan は未使用と確認
- config 読み込み順、全 provider 巡回、設定済み項目・profile の再確認、agent 二重対話を
  根因として Issue #262 と `.agent/investigations/issue-262-setup-ux.md` に記録

## 2026-07-15（#262: setup UX 方針 A/B 実装）
- 認証済み・導入済み provider だけをプラン解決対象にし、detected / previous / default の
  優先解決を tako-control に集約。検出値の食い違いは detected 優先で通知
- config を質問前に読み、2 回目 Enter で agent / plan / profile を引き継ぐ冪等経路を追加。
  claude 単独・3 CLI の隔離 E2E で追加質問 0・setup agent 再起動なしを確認

## 2026-07-15（#262: setup UX 方針 C/D/E 実装・検証）
- 標準 setup を最終サマリだけの質問ゼロへ変更し、`--yes` / 全項目 `--answers` /
  dispatch `SetupRun` / MCP `tako_setup` と明示 `--review` を実装
- 初回・2回目・`--yes`・未認証を before 5+/5+/未実装/1 → after 全 0 入力で実測。
  実 Claude Max 認証、検出競合、破損 config、複数 CLI、全品質ゲートも全緑

## 2026-07-17（#307: 左サイドバーのドラッグリサイズ）
- 右端リサイズハンドル + ドラッグ追従 + 120px〜50% クランプ + settings.json 永続化 + CLI/MCP 1:1。右パネルと同方式
- 関連コミット: PR #316 squash merge（`e961acc`）。worktree 掃除・Issue 証拠コメント済み
- 次: ユーザー目視確認（カーソル変化・ドラッグ・永続化）→ #307 クローズ

## 2026-07-17（#312: macOS ウインドウ操作の不備修正）
- タブバードラッグ移動（`start_window_move`）+ ダブルクリックズーム + 赤ボタン close 後の Dock 復帰（`on_reopen` + `on_window_should_close`）
- 関連コミット: PR #318 squash merge（`9cd2535`）。worktree 掃除・Issue 証拠コメント済み
- 次: ユーザー目視確認（ドラッグ移動・Dock 復帰・既存操作との競合なし）→ #312 クローズ

## 2026-07-17（#315: PDF プレビューのリンク ⌘クリック無反応を根治）
- 根因 = estimate_pdf_page_bounds がテキストなしページで None → ヒットテスト常時失敗。canvas paint でページ画像 bounds を直接記録する方式に変更 + 全描画ページチェック + ホバー全ペイン化 + カーソル変化 + 下線ハイライト
- 関連コミット: PR #323 squash merge（`9373003`）。worktree 掃除・Issue 証拠コメント済み
- 次: `build-app.sh --install` → ユーザー目視確認 → #315 クローズ

## 2026-07-17（アプリ内 Web ビュー品質監査 — 調査・起票のみ、コード変更なし）
- 隔離インスタンス（TAKO_ISOLATED=1 + release）で CLI/MCP 経由に全経路検証。最重大: 空白入り等 NSURL 不能な URL を web open/nav に渡すと wry の `NSURL::URLWithString().unwrap()`（mod.rs:826）で panic → tako 全体 abort（全ペイン消失）。#325/#326/#327 起票
- 正常確認: URL 正規化・back/forward/reload・hide/show の SPA 維持・file:// のクロスオリジン fetch ブロック・persist 復元・close 後始末・data:/IDN
- 未検証（別スペースで GUI 操作不可）: cmd+K 等キーボード衝突・実クリック/IME/テーマ目視 → 報告書メモに記載

## 2026-07-17（#324: sleep-guard の busy_agents が復元 worker を数えない問題を根治）
- 根因 = `update_sleep_guard()` が OSC 133 の `CommandState::Running` のみカウント。persist 復元後は `Unknown` のまま遷移しないため常に 0。`Unknown` バックエンドセッションの子プロセスをバッチ判定し busy にカウントする修正
- 関連コミット: PR #328 squash merge（`f685e27`）。worktree 掃除・Issue 証拠コメント済み
- 次: `build-app.sh --install` → 暫定運用復帰 → ユーザー実機確認 → #324 クローズ

## 2026-07-17（#313: git タブがファイルツリーの表示リポジトリに追随しない問題を根治）
- 根因 = git タブが `active_tab_cwd()`（フォーカスペインの cwd のみ）を参照。ファイルツリーは全ペイン cwd + pinned フォルダを集約するが git タブはこのソースを見ていなかった。`git_cwd_for_tab()` を新設しフォールバック検索に変更
- 関連コミット: PR #331 squash merge（`2606d03`）。worktree 掃除・Issue 証拠コメント済み
- 次: `build-app.sh --install` → ユーザー実機確認 → #313 クローズ

## 2026-07-17（#319: worker の permission ダイアログ検知 + 構造化応答 API）
- `detect_permission_dialog()` で画面から permission ダイアログを構造化検知（コマンド・選択肢・ハイライト）。`worker_status` に `permission_dialog` フィールド、watch に `WORKER_PERMISSION` イベント、`OrchestratorRespond` + CLI `respond` + MCP `tako_orchestrator_respond`（95 ツール）を追加。master prompt に安全/危険コマンドの承認規範を追記
- 関連コミット: `f8f4dc0`（PR #344）。build / fmt / clippy / test 全緑（484 + 278 passed）
- 次: 隔離セルフテスト + 実 claude ダイアログ実測 → squash merge → #319 クローズ

## 2026-07-17（#333: エラーレポートの自動送信基盤 — テレメトリ）
- Cloudflare Worker + Rust telemetry + panic ハンドラ + CLI/MCP 1:1（95 ツール）。既定 OFF（opt-in）。Worker デプロイ + 通し実測（人工レポート到達確認）完了
- 関連コミット: PR #345 squash merge（`a19dd54`）。Worker テスト 11/11 + Rust テスト 12/12 + 品質ゲート全緑
- 次: `build-app.sh --install` で反映 → #333 クローズは master 判断

## 2026-07-17（#338: プレビューペインにチェンジログビュー切替を実装）
- 「履歴」トグルでコード ⇔ git 履歴ビュー切替。コミット一覧 + ファイル単位 diff 展開。git 管理外ファイルは安全表示。CLI `tako preview-changelog` + MCP `tako_preview_changelog`（95 ツール）
- 関連コミット: `f2c1c80`、PR #348。テスト 4 本追加（606 全緑）+ 品質ゲート全緑
- 次: squash merge → `build-app.sh --install` → 実機確認 → #338 クローズは master 判断

## 2026-07-17（#314: ファイルツリー右クリメニュー改善 — デフォルトアプリ/見切れ修正）
- ファイル右クリに「デフォルトアプリで開く」「このアプリで開く...」追加 + コンテキストメニューの見切れ修正（フリップ/クランプ）。FileOpKind に OpenDefault / OpenWith 追加（dispatch / MCP / CLI 1:1）
- 関連コミット: `495ca44`（PR #349 squash merge）。テスト 5 本追加（486 全緑）+ 品質ゲート全緑 + 隔離セルフテスト完走
- 次: `build-app.sh --install` → 実機確認 → #314 クローズ

## 2026-07-17（#320: シンタックスハイライト対応形式の大幅拡充）
- two-face crate（bat 由来）を導入し 75→210+ 構文。TOML・Dockerfile・TypeScript・Swift・Kotlin・CMake 等が新規対応。ファイル名判定 + 拡張子フォールバック追加。バイナリ +550KB（2.5%）
- 関連コミット: `5dd4bd5`（PR #351 squash merge）。テスト 7 本追加 + fmt / clippy / test 全緑
- 次: `build-app.sh --install` → 実ファイルでの色分け目視確認

## 2026-07-17（#338: プレビューペインにチェンジログビュー切替を実装）
- 「履歴」トグルでコード ⇔ git 履歴ビュー切替。コミット一覧 + ファイル単位 diff 展開。CLI `tako preview-changelog` + MCP `tako_preview_changelog`（97 ツール）。隔離セルフテスト完走（FAILED 0 件）
- 関連コミット: `d29005d`（PR #348 squash merge）。テスト 4 本追加 + 品質ゲート全緑
- 次: `build-app.sh --install` → 実機確認（目視チェックリスト）→ #338 クローズは master 判断

## 2026-07-17（#357: codex / agy の利用制限データ取得）
- codex TUI の `primary NN%` / `secondary NN%` スクレイピング + サービス別メトリクス保持 + ドロップダウン実データ表示。agy は取得不能で「--」維持（調査結果を Issue にコメント）
- 関連コミット: `690d220`（PR #359 squash merge）。テスト 4 本追加 + 品質ゲート全緑
- 次: `build-app.sh --install` → 有料プラン codex 環境での実測確認（free tier では rate limit 表示なし）

## 2026-07-17（#282: remote 刷新 弾3 — Tailscale transport 一本化・統合ブランチ開始）
- `renewal/remote-transport` を開始。`tako-control::tailscale` 新設（検出 / setup 判定 /
  serve 管理。判定関数は弾 6 と共有）、daemon を tailscale serve 化（固定 ts.net URL・
  未 setup は不足列挙 + `tako remote setup` 誘導で停止）、cloudflared / relay / --insecure /
  `web/tako-remote-worker/` / setup 依存チェックの cloudflared を全削除
- 副産物修正: daemon_status の 3 行 PID 未追従（常に running=false）/ spawn_daemon の
  PATH 旧バイナリ化け / 子 stderr 握りつぶし / agents.rs の clippy 違反（いずれも main 由来）
- 検証: 全品質ゲート + 隔離セルフテスト完走 + 未 setup 4 状態・serve 残骸エッジの実測 +
  実 tailnet 通し実測（start → TLS 検証込み到達（health/PWA/認証/WS 101）→ stop → 到達不能。
  受け入れ 1〜4 完了、証拠 = Issue #282 コメント。iPhone 実機到達のみ要実機確認）
- 次: 弾 4（#283 ペアリング認証 + PWA daemon 配信）を同ブランチに積む

## 2026-07-17（#287: セキュリティレビュー P1×2 + P2×4 修正）
- P1-1 XFF identity 偽装対策（XFH 検証追加）、P1-2 PII プレースホルダ化、P2-1〜P2-4（upload 0o600 / 監査ログファイル名除去 / Windows パス redact / symlink 拒否）。threat model 正確化。テスト 9 本追加、全 843 テスト緑
- 関連コミット: `d008e6c`（`renewal/remote-transport`）。Issue に修正証拠コメント済み
- 次: master レビュー → main マージ判断 → 実 serve 経由 e2e → v0.6.0

## 2026-07-18（#321: ステータスバーのサービス切替ドロップダウン無反応を根治）
- 根因 = ステータスバーの container div の overflow_hidden がポップアップをクリップ。メニューをルート div のオーバーレイへ移動（#346 コンテキストメニューと同方式）。背面 dismiss 追加
- 関連コミット: `09fde57`（PR #361 squash merge）。fmt / clippy / test 288 / セルフテスト全緑
- 次: `build-app.sh --install` → 実機目視確認

## 2026-07-18（#312 再修正: 赤ボタン close → Dock 復帰でタブが空になるバグを根治）
- 根因 = PRIMARY_CLAIMED（#113 多重起動ガード）が swap(true) のみで false に戻す処理がなかった。赤ボタン close → プロセス生存 → on_reopen → TakoApp::new 2 回目 → セカンダリ判定 → 復元スキップ。release_primary() を新設し on_window_should_close で解放
- 関連コミット: `5c1795e`（PR #362 squash merge）。fmt / clippy / test 288 / セルフテスト全緑
- 次: `build-app.sh --install` → 実機確認（赤ボタン close → Dock 復帰 → タブ完全復元）

## 2026-07-18（#308 再修正: タブ D&D がウインドウ移動に食われる競合を根治）
- 根因 = GPUI の on_drag は DRAG_THRESHOLD(2px) 超過まで待機するが、親 tab-bar の on_mouse_move → start_window_move() が 1px 移動で先に発火。tab_mouse_down フラグで抑制
- 関連コミット: `73da200`（PR #363 squash merge）。fmt / clippy / test 288 / セルフテスト全緑
- 次: `build-app.sh --install` → 実機確認（タブドラッグ並べ替え・空き領域ウインドウ移動の両立）

## 2026-07-18（#338 再修正: チェンジログビューの git 検出が .app 環境で全滅する問題を根治）
- 根因 = `repo_root()` に `current_dir` としてファイルパスを渡すと ENOTDIR で git 実行不能。横断で `"git"` 直接指定 3 箇所も `git_bin()` に統一
- 関連コミット: `4395f32`（PR #365 squash merge）。fmt / clippy / test 954 全緑。回帰テスト 4 本追加
- 次: `build-app.sh --install` → Dock 起動で履歴トグル・ファイルツリー git マークの目視確認

## 2026-07-18（#364: orchestrator report — scrollback + transcript 2 層実装）
- 第 1 層 tmux scrollback（capture-pane -p -J -S）全 agent 共通 + 第 2 層 claude transcript アダプタ。
  CLI `tako orchestrator report` + MCP `tako_orchestrator_report`（100 ツール）。worker-status に
  history フィールド追加。隔離実測で可視画面 10 行 → scrollback 330 行を確認
- 関連コミット: `46b925b`（PR #366 squash merge）。288 tests / セルフテスト FAILED 1（既知 #332）
- 次: `build-app.sh --install` → 実 claude ペインでの report e2e + codex fallback 実測

## 2026-07-18（#339: 複数ウィンドウ対応 — ビューポート方式）
- 単一 TakoApp entity を全 GPUI ウィンドウの root view として共有（GPUI の entity→複数 window
  invalidation を確認して採用）。tako-core に WorkspaceWindow/WindowId + タブ排他割当、
  CLI `tako window` + MCP `tako_window`（101 ツール）+ persist windows[]（後方互換）
- 検証: 品質ゲート全緑（978 tests）+ 隔離セルフテスト完走（項目 77 新設）+ 隔離実測
  （2 窓別タブ send/read・move-tab・再起動復元・orchestrator spawn→WORKER_IDLE）
- 関連コミット: `52bb49d`（PR #367 squash merge）。Issue に実測証拠コメント済み
- 次: `build-app.sh --install` → 実機目視（New Window の状態同期・赤ボタン Dock 復帰）
## 2026-07-18（#340: エネルギー・CPU 監査 — 3 状態実測 + 棚卸し + sleep_guard BG 化）

- 実測: アイドル 0.24% / 通常 1.05% / 高負荷(8 ペイン 160 行/秒) 1.64% = 本体は軽量。
  見えない消費 2 件を特定: claude agents 5s スキャン(0.2s CPU/回 = 1 コア 4% 相当 → #368)、
  sleep_guard の UI 専有 p50 42ms×毎 2s(#324 由来 → 本タスクで BG 化)。pane_log probe は #369
- 常駐ポーリング棚卸し表 15 項目 + 残骸プロセス(5.8 日常駐 headless Chrome 等)を Issue #340 に報告
- 関連コミット: PR #370 squash merge（`3cd5693`）。before p50 50ms → after 0ms(隔離再現ベンチ)
- 次: #368 / #369 の着手判断、残骸プロセスの掃除はユーザー判断

## 2026-07-19（#371: タブ D&D 挿入位置インジケータの実装）
- ドラッグ中に挿入位置を示す縦線バー（3px + accent glow）表示、ソースタブを半透明 + 点線ボーダー化。scroll-area `on_drag_move` によるインジケータ上書き問題を修正
- 関連コミット: `55f9c31`（PR #373 squash merge）。隔離実機ダーク/ライト両テーマ + セルフテスト全緑
- 次: `build-app.sh --install` → 本番目視確認

## 2026-07-19（#378: タブ名の自動命名 — source パラメータ + 命名規則設定 + プロンプト追記）
- tako_rename_tab に source パラメータ追加（auto = set_title_auto で手動を上書きしない / manual = 従来）。
  Profile に tab_naming_convention 追加（profiles set --tab-naming-convention / MCP 1:1）、
  master / solo プロンプトにタブ名更新の標準動作を追記。テスト 4 本追加・品質ゲート全緑
- 関連コミット: `6948491`（PR #382 squash merge）。Issue #378 close 済み
- 次: `build-app.sh --install` → 実機で source=auto と命名規則注入の確認

## 2026-07-19（#375: Web dock URL 入力欄のフォーカス不在を修正）
- 根因 2 件: ①フォーカス時カーソルバー非表示（空入力でプレースホルダのみ） ②フォーカス解除欠如（Enter/Escape 以外で false に戻らない）。カーソルバー方式に刷新 + ルート div の on_mouse_down でフォーカス解除
- 関連コミット: `071f37e`（PR #383 squash merge）。品質ゲート全緑（308 tests）
- 次: `build-app.sh --install` → ユーザー実機でカーソル表示 + 手打ち + IME 確認 → #375 クローズ判断

## 2026-07-19（#315 R2: PDF リンク cmd ホバー/クリック不発を根治）
- 根因を計装で確定: render のたびに `preview_pdf_page_image_bounds` を空 HashMap でクリアし、canvas paint の `cx.defer()` で再記録する設計のタイミング窓。release ビルドの .app 環境で GPUI effect cycle の差異により常に空 map を参照。render 冒頭のクリアを削除（defer 上書きで実害なし）
- 関連コミット: `f40bcc0`（PR #386 squash merge）。品質ゲート全緑（308 tests）+ セルフテスト PDF 全通過
- 次: `build-app.sh --install` → ユーザー実機で cmd ホバー + cmd クリック確認 → #315 クローズ判断

## 2026-07-19（#369 + #374: orchestrator 改善 — probe 一括化 + report --messages）
- #369: `pane_log_probe_batch()` 新設。2 秒 tick の tmux 起動を N 回→1 回に削減（list-panes -a -F）。テスト 2 本追加
- #374: `tako orchestrator report --messages N` + MCP `messages` パラメータ。直近 N 件 assistant テキスト取得（古い順、既定 1、超過=全件）
- 関連コミット: `b3f1bbc`（PR #387 squash merge）。品質ゲート全緑（983 tests）
- 次: `build-app.sh --install` → 実 claude ペインで `--messages 3` の取得実測

## 2026-07-19（#372: sleep-guard busy_agents 漏れの根治）
- 旧実装は Unknown ペインのみ子プロセス判定 → Idle のまま子プロセスが走る TUI エージェントを見落とし。全バックエンドを対象に変更 + status() の busy_agents ハードコード 0 も修正
- 関連コミット: `f652dc8`（PR #389 squash merge）。308 tests / clippy / fmt 緑 + 隔離セルフテスト完走
- 次: `build-app.sh --install` → 本番で busy_agents 確認。Issue クローズは master 管理

## 2026-07-19（#357: 利用制限表示にリロードボタン追加 + agy unsupported 明示）
- USAGE LIMITS ドロップダウンのヘッダーにリロードボタン追加（即時再走査）、agy を「unsupported」明示表示に変更。dispatch / CLI / MCP に refresh アクション追加（1:1）。codex のローカル DB 調査で永続化なし確認
- 関連コミット: `3059701`（PR #393 squash merge）。310 tests / clippy / fmt 緑
- 次: `build-app.sh --install` → リロードボタン実機確認

## 2026-07-19（#391: setup 対話 agent の既定起動を復元）
- 回帰点を特定: #322（PR #330）で master 提案優先ロジック追加時に agent フォールバック欠落 + `--review` 限定化。修正 = 旧ランチャー除去 + `launch_setup_agent` を既定呼び出し。`--yes`/非TTY/`launch_agent=none` でスキップ。docs に「素のコマンドで完結」原則を追記
- 関連コミット: `15fefa2`（PR #396 squash merge）。CLI 27 + control 529 + core 310 tests / clippy / fmt 緑 + 実機で対話起動・greeting 注入を実測
- 次: `build-app.sh --install` → 本番で `tako setup` の対話起動確認

## 2026-07-19（#381: 赤ボタン close → Dock 復帰の TakoApp 二重生成による全タブ消失を根治）
- 根因を実測で確定: on_reopen の TakoApp::new 再生成で旧 entity がゾンビ化（本番 pid で IPC ソケット
  2 個 LISTEN を実測）→ 復元 spawn -A -D がゾンビの tmux クライアント強奪 → Exited 連鎖で
  「まっさら」（1 回目）/ cx.quit() の silent death（2 回目、隔離で再現）。修正 = PrimaryApp global +
  reopen_or_restore で同一 entity のウィンドウ開き直し + GPUI 枚数判定 + panic.log 常時記録
- 検証: 根因シーケンス A/B 比較（修正前 = プロセス即死、修正後 = タブ・クライアント・ソケット維持）+
  受け入れ 3 パターン（New Window 直後 / タブ移動後 / ウィンドウ close 後の quit → 再起動で全タブ復元）+
  persist OFF エッジ + 品質ゲート全緑。復元の複数ウィンドウ開き直しはクリーン 3/3 決定的
- 副産物: 検証 CLI が worker 注入 env（TAKO_SOCKET）で本番ゾンビに誤接続し layout.json を一時汚染
  （即時復旧済み、E2 の自然保存で 5 タブ復帰確認）。以後の隔離検証は env -u ラッパー必須

## 2026-07-19（#381 完了: PR #400 squash merge）
- 全根因の実測確定（TakoApp 二重生成 → ゾンビ → -A -D 強奪 → まっさら/silent death）と
  entity 再利用への構造修正 + 堅牢化 3 点（panic.log 恒久記録 / orphan 復帰総点検 +
  catch_unwind / 空 layout 保存拒否 + layout.json.good + recover --apply good）
- 関連コミット: `f9c2ac8`（PR #400 squash merge）。受け入れ 3 パターン + panic e2e +
  orphan 反復 ×7 隔離実測。証拠は ~/Desktop/tako-381-evidence/
- 次: #380 共有タブバー化（同一 worker 継続）。実機 install はユーザー

## 2026-07-21（#380 完了: タブバーの全ウィンドウ共有化）
- タブバーを全ウィンドウ共有に是正（全タブ表示 + クリックで表示奪取 + W バッジ + 巡回全タブ化。
  排他原則・move-tab CLI/MCP 互換・persist 復元は維持）
- 関連コミット: `823e149`（PR #402 squash merge）。実クリック奪取・persist 復元・品質ゲートを
  隔離実測。スクショ証拠は ~/Desktop/tako-380-evidence/
- 次: build-app.sh --install → ユーザー実機確認（#381/#380 とも）→ Issue クローズは master

## 2026-07-21（renewal 統合同期: origin/main → renewal/remote-transport マージ）
- v0.6.0 前の統合同期。main 47+ コミット（#391/#358/#384/#392/#397/#398/#381/#380/#399 等）を
  3 段マージ（`aa349ae` → `a910886` → `e8a7165`）で取り込み。コンフリクト 13 ファイル解決
  （remote.rs = renewal 正 + #384/#330 統合、setup.rs = #391 と remote 案内両立、
  changes.yaml = rev 12 振り直し、MCP スナップショット 103 ツール再生成）
- 検証: fmt / clippy(-D warnings) / test 全緑（1028 本）+ main 側 20 PR の消失ゼロを機械確認。
  セルフテスト「worker_status IPC（#181）」FAILED は素の main で再現する main 由来（#390 対処中）
- 次: iPhone 実機確認（統合ビルド）→ #287 レビュー → renewal → main 逆マージ → v0.6.0

## 2026-07-21（#390: worker レジストリ — ペイン消失・突然死後の追跡継続）
- workers.yaml 永続レジストリ新設（spawn 登録 / close 記録 / sid 自動昇格 / GC）。watch / status /
  report は pane 消失時にレジストリの tmux_session / session_id で自動補完（判定ロジック不変の
  フォールバック層）。prompt 未達検知（240 秒 + 非 busy → prompt_undelivered）、SIGSEGV 突然死
  検知（WORKER_DEAD + claude --resume 提示、自動 resume は設計判断で見送り）、PromptFlow 再貼り付け、
  pane ID 再利用の誤マッチ防御、`tako orchestrator workers` + MCP（計 104 ツール）
- 関連コミット: `b23266d`（PR #408 squash merge）。受け入れ 1〜3 + 突然死を隔離実測（実 claude、
  kill -9 再起動 / 送達阻害 / kill -SEGV）。副産物: セルフテスト 74 の確定失敗を修正同梱、
  #406（webview close 失敗）/ #407（Bypass ダイアログ死）起票
- 次: `build-app.sh --install` → 本番 master での workers / --worker 運用開始

## 2026-07-22（#413: タブ D&D インジケータが右端固定になる回帰を修正）
- 根因: GPUI の `on_drag_move` は capture フェーズで全登録要素に hitbox チェックなしで発火し、
  DOM 順で最後の + ボタンが常に勝ちインジケータが末尾固定。#371 でスクロールエリアの上書きは
  除去したが + ボタンの上書きは残っていた。#402 の全タブ描画化で視覚的に顕在化
- 修正: 各 `on_drag_move::<TabDrag>` に `bounds.contains(&position)` チェックを追加
- 関連コミット: `5bf0759`（PR #419 squash merge）。品質ゲート全緑 + セルフテスト FAILED 0
- 次: `build-app.sh --install` → 実機でタブ D&D のインジケータ正位置を目視確認

## 2026-07-22（#421: セルフテスト type_text ハングの根治）

- 根因: `dispatch_keystroke` が毎文字 GPUI フルレイアウト再計算（taffy flexbox）をトリガー。
  テスト 69c の `link_command`（182 文字）で 182 回再描画→タイムアウト。`sample` でスタック確定
- 修正: `type_text` に 80 文字閾値を導入。長い文字列は PTY 直接 `paste()` で再描画を回避
- 関連コミット: `f0a3a6c`（PR #422 squash merge）。品質ゲート全緑（1080 tests）

## 2026-07-22（#423/#426/#424: リモートサーバーバグ 3 件修正）

- 根因: v2 API の数値 PaneId を WS/screen API に渡すと tmux ターゲットとして無効で即エラー → WS 無限 open/close + term 無限ロード。PaneId→tmux ターゲット自動解決を全 API に追加、WS 通知デバウンス、v2 fallback 統一
- 関連コミット: `eea56e1`（PR #427 squash merge）。品質ゲート全緑（1080 tests）
- 次: `build-app.sh --install` → 実機でリモート接続検証

## 2026-07-22（#425: リモート承認カード誤表示の修正）

- 根因: transcript 正規化で最終 assistant の tools を無条件に approval と判定。auto mode で自動実行された全コマンドに承認カードが表示されていた
- 修正: has_pending_tools フラグで tool_result 到着を追跡、未到着のみ approval 付与。テスト 4 本追加
- 関連コミット: `b364bcd`（PR #430 squash merge）。品質ゲート全緑（1084 tests）
- 次: `build-app.sh --install` → 実機確認（auto mode 非表示 + 実ダイアログ表示）

## 2026-07-22（#432/#426/#428/#424/#429: リモート実機 FAIL の徹底調査 + 修正）

- 根因を隔離実測で確定: ①input が "session:0.0" を dispatch tmux_session へ渡し `=session:0.0:`
  で can't find pane 無音失敗（#428）②WS init が term DOM 未マウント中に捨てられ、開き直しは
  init キャッシュ 0ms 着弾で必ず無限ロード（#426/#428）③serve_binary が current_exe 優先で
  PATH 上の dev CLI から旧世代 serve が立つ（#432。#424 表示欠落もこれ起因の疑い）
- 修正: input の PaneId 直渡し / PWA 保留 init / Enter=改行・cmd/ctrl+Enter=送信（#429）/
  serve_binary の /Applications 優先 + status 可視化 + 世代食い違い検知
- 検証: 隔離 serve + Playwright で before/after 実測（送達成功・2 回目表示・master 一覧表示）

## 2026-07-21（#438: リロードアイコンを円弧+三角矢じりへ再デザイン）

- 前回却下案（円弧2本+Lブラケット矢印）の敗因を踏まえ、単一 270°円弧 + 塗り三角矢じり
  （Chrome/Material 同型）で作り直し。候補 4 案の実レンダリング比較 + 12px@2x ピクセル拡大で
  採用案を決定、隔離実機の before/after 実寸スクショで ✕ に見えないことを目視検証
  （証拠 ~/Desktop/tako-438-evidence/）。使用 2 箇所は同一アセット参照で統一
- 関連コミット: `025bad7`（PR #441 squash merge）。品質ゲート全緑 + セルフテスト FAILED 0
- 次: `build-app.sh --install` → 実機で見た目確認 → #438 クローズはユーザー判断

## 2026-07-21（#287 P1: 監査文書の実 PII をプレースホルダ化）

- codex 公開前レビュー P1 所見の是正。`reviews/2026-07-06_公開前セキュリティ監査.md` と
  `.agent/plans/security-audit-report.md` の実ユーザー名・ホスト名・ホームパス・実メール・
  実名入り旧 URL を `<user>`/`<hostname>`/`<email>`/`<account_id>`/`<redacted-url>` へ置換（文意不変）
- 関連コミット: `a9fd9c8`（PR #447 squash merge）。grep で実値ヒット 0 件を確認、Issue にコメント済み
- 残: #287 の P1-1 cross-origin / P1-2 identity spoof（実装修正）と履歴側 PII の扱い決定は別タスク

## 2026-07-22（#440: sleep-guard チップの平易化 + 詳細ポップオーバー + i18n-ready 化）

- 「awake+lid」を平易な 3 状態（スリープ防止中 / ・蓋閉じOK / ・高温注意=赤字）へ刷新、
  クリックでモード・理由（エージェント N 体稼働中）・蓋閉じ挙動・変更方法（クリックコピー）の
  ポップオーバー新設（#361 のルートオーバーレイ方式）。文字列は ui_text.rs 新設カタログへ
  集約（#435 案 B のキー化準備）。☕ は #217 で SVG 化済みを grep 確認（UI に絵文字ゼロ）
- 検証: 候補 9 案の実寸レンダリング比較 + 実クリック e2e（開閉 / backdrop dismiss / コピー
  pbpaste 実測）+ unit 14 本 + 品質ゲート全緑。セルフテスト FAILED 1 = #332 のみ
  （素の main で同一再現 = 差分ゼロ）。証拠 ~/Desktop/tako-440-evidence/

## 2026-07-22（#439/#425: リモート master のチャット非検出 + auto mode 承認カード誤表示を根治）
- #439: agent_type 判定を role 依存 → live claude 解決（`claude agents --json` の pid 祖先辿り）へ。
  role 空 master でも対話型 claude 稼働なら claude 化 + session_id 付与（agents::live_claude_sessions_by_backend 新設）
- #425: 承認判定を transcript 推定 → 画面 permission ダイアログ実在へ再設計（transcript approval 全廃）。
  v2 panes に permission_dialog 付与 + POST /api/panes/:id/respond 新設。command 抽出を罫線ボックス内に限定
- 関連コミット: `559ccca`（PR #444 squash merge）。隔離実測 before/after 完了、実機確認まで Issue オープン維持
- 副産物: 隔離検証の TAKO_REMOTE_STATE_DIR 隔離漏れで本番 remote state ファイル破壊 → #445 起票
  （本番 daemon は生存・実サービス無傷、復旧は実機アイドル時に `tako remote start` で旧 daemon 自動回収→再生成）

## 2026-07-22（#287 P1: cross-origin での terminal 読取・操作を遮断）

- REST/WS の Origin を daemon base_url と完全一致検証、不一致は認証より手前で 403 拒否。
  CORS `*` を廃止し許可 origin のみエコー + Vary: Origin。WS で subprotocol 必須化。テスト 5 本追加
- 関連コミット: `f12a9af`（PR #450 squash merge）。品質ゲート全緑 + 隔離デーモンで evil/正規 Origin の e2e 実測
- 残: P1-2 identity spoof（Unix socket 化）は別タスク

## 2026-07-22（#435: UI の日英 i18n — ロケールキー化 + 言語切替 UI/CLI/MCP）

- `tako-core::i18n` 新設（OS ロケール検出 = env→AppleLanguages→英語）+ `ui_text/` を
  `tr!(ja, en)` カタログ化し主要 UI を英語対応。切替は CLI `tako lang` / MCP `tako_lang`
  （106 ツール）/ パレットの 3 経路（settings.json `language` 永続化・GUI 即時反映）。
  conventions.md に「新機能は日英必須」を明文化
- 関連コミット: `be0606d`（PR #454 squash merge）。品質ゲート全緑 + 隔離セルフテスト
  FAILED 0（33c = MCP lang e2e 新設）+ 隔離実測（日英切替スクショ・OS ロケール既定・永続化）
- 次: install 後に実機確認（`tako lang en` → 英語 UI・パレット切替）→ #435 タスク 3
  （README 英語化）は別タスク

## 2026-07-22（#459: 設定画面（Cmd+,）の詳細設計 — 設計のみ・コード変更なし）

- `.agent/plans/2026-07-settings-ui.md` 新設: タブ 7 構成 / 色設定スキーマ（Theme 58 色を
  settings.json `theme_colors` + `theme_presets` で上書き、既存 `Theme` dispatch の action 拡張）/
  独立 GPUI ウィンドウ（別 root view + pending キュー）/ dispatch 直呼びによる CLI・MCP 1:1 /
  M1〜M7 マイルストーン（機械検証つき受け入れ条件）
- 調査で確定: 設定変更 dispatch 15 種の一覧、settings.json 17 フィールド、MCP 106 ツール
  （snapshot 実測）、confirm_close のみ非永続（config.yaml 初期化・要修正）
- 次: master レビュー → M1 から実装着手（worker 割当は master 判断）

## 2026-07-22（#453 M4: Code Runner UI — 再生ボタン + プロファイルドロップダウン）
- プレビューヘッダに play.svg アイコン + ドロップダウンシェブロン（2+ プロファイル時）を追加。
  宣言検出 / 拡張子既定 / 淡色無効 / dirty 自動保存 / dispatch Run 経路を実装。i18n 対応
- 関連コミット: PR #460 squash merge。品質ゲート全緑（359+27 tests / fmt / clippy）。
  CLI 検証 4 項目パス。GUI スクショは screencapture 権限の制約で未取得（ユーザー実機確認待ち）

## 2026-07-22（#453: 再生ボタン無反応の根治 — 復元経路の検出漏れ + Run ペイン即死）
- 根因 2 件を隔離実測で確定: ①persist 復元・リロード経路で preview_run_profiles 未検出
  （ボタン淡色 = on_click 無し）→ detect_preview_run_profiles 抽出 + 3 経路呼び出し
  ②spawn_command_pane の複合コマンド program 1 語詰め → login_shell_command クォートで
  127 即死 → /bin/sh -c 構造化。テスト 2 本追加 + 既存 1 本を正しい構造検証へ更新
- 検証: before/after 隔離実測（profiles None→Some(1)・Run ペイン即死→出力可視）+
  品質ゲート全緑（1195 tests / fmt / clippy）

## 2026-07-22（#466: リモートチャットビューの更新停止を根治 — sticky live 解決 + カタログ最新優先）

- 根因を隔離実測で確定: ビュー切替はシロ（モック/実環境 × Chromium/WebKit で再現せず）。
  真因 = `claude agents --json` の一時失敗・列挙漏れ（実測: ps 7 プロセス中 1 個欠落）で
  live 解決が消えると、v2 panes の session_id がカタログの stale 旧世代（同一 pane に
  20 世代超・辞書順先勝ち）へ化け、チャットが凍結 transcript を読み続けていた
- 修正: agents.rs sticky live 解決（失敗・欠落時は直近成功値を保持、ペイン消滅で破棄）+
  sessions.rs resolve_session_for_pane の last_seen_at 最新優先。テスト 6 本追加
- 検証: 隔離 GUI + 実 claude 2 世代 + fail 注入で before 凍結 / after 継続（切替 5 ラウンド）
  を実測。品質ゲート全緑（fmt / clippy / 1005+ tests）

## 2026-07-22（#470 Phase A: 紹介動画の構成台本 + 収録パイプライン検証）
- 構成台本 `.agent/plans/2026-07-promo-video.md`（強み訴求順 6 項目・シーン 7 本・計 72 秒）+
  収録スクリプト `scripts/promo/record-sample.sh`（隔離インスタンス + CLI ビート再生 +
  screencapture -v）。サンプル 15 秒 mp4 を実収録し全 15 フレーム目視で PII ゼロを確認
- 関連コミット: `f7442ef`（PR #474 squash merge）。クリップは ~/Desktop/tako-promo/（リポ外）
- 次: ユーザーの構成承認（尺・トーン・訴求順・S6 収録方法）→ Phase B 全シーン収録

## 2026-07-24（#470 紹介動画 v2: テロップ背景 + setup / master 節の新設）
- テロップを半透明パネル付きに刷新（v1 は背景 UI と重なって読めなかった）。本編を
  画面操作 → プレビュー → setup → master の 4 本柱へ再構成し、setup 節と master 節を新規収録
- 収録の落とし穴を発見・対処: 収録ウィンドウが他ウィンドウに完全に隠れると GPUI が描画を
  止め、同じ絵が撮れ続ける。定期 activate + 「異なるフレーム数」チェックで自動検出するようにした
- 完成品 `~/Desktop/tako-promo/tako-intro-v2.mp4`（84s / 1920x1200 / BGM）。全 168 フレームを
  Vision OCR にかけ PII ゼロを確認

## 2026-07-24（#470 紹介動画 v3: setup 節を対話セットアップの訴求へ + master にプロジェクト文脈）
- setup 節を「コマンド紹介」から「対話セットアップエージェントと会話して設定が決まる」へ作り直し、
  実 claude の対話を新規収録。master 節の直後に S6c（ホーム起動 master が projects.yaml から
  対象プロジェクトを解決し、その cwd で worker を立てる）を追加。訴求は setup.rs /
  system-prompt.md / default_system_prompt.md Step 0 / dispatch.rs で裏取りした範囲だけ
- 収録知見 3 件を台本へ追記: デモ HOME だとログインキーチェーンが検索リストから外れて
  claude が認証できない / `tko` に TAKO_DATA_DIR を渡さないと本番設定を書き換える /
  `--await-prompt` は生成中のエージェントを中断させることがある
- 完成品 `~/Desktop/tako-promo/tako-intro-v3.mp4`（106s / 1920x1200 / BGM）。全 106 フレームを
  Vision OCR にかけ PII ゼロ

## 2026-07-24（#487: git タブ総点検 + ファイル単位ステージング UI）
- 隔離 GUI で全操作を実クリック監査（所見 21 件）。VSCode 相当の 2 セクション + 行 ±ボタン +
  一括ステージ + 更新ボタンを追加し、コミット入力欄の文字入力不能（IME 経路欠落・大文字小文字化・
  フォーカス握りっぱなし）、`commit -a` 固定、初期コミット diff 誤り、横見切れ、svg 不可視を修正
- 関連コミット: `1354b1a`（PR #492 squash merge）。品質ゲート全緑（1214 tests）+ install 済み。
  証拠は ~/Desktop/tako-487-evidence/
- 次: セルフテスト項目 33b（テーマ）が main 由来で失敗するため別途起票が要る

## 2026-07-25（#494: git タブ描画崩壊の根治 + 操作の堅牢化）
- 根本原因は幅ではなく「コンテンツ総高さ > パネル高さ」。1 枚の flex 列 + overflow_y_scroll
  だったため taffy が行を圧縮し、overflow_hidden の行は高さ 0 付近まで潰れ、visible な行は
  隣へはみ出して重なっていた。固定ヘッダ + 行が縮まないスクロール本文の 2 段構造へ分離
- 堅牢化: 空メッセージ・変更ゼロはボタン / Cmd+Enter 両方で拒否 + 理由表示、実行中は
  ボタン無効化、失敗は閉じるまで表示、制御文字の正規化と 4096 バイト上限、
  キャレットの文字境界丸め（split_at / insert_str の panic を構造排除）
- 副産物: porcelain v2 の `u`（unmerged）未対応でコンフリクト中に「変更はありません」に
  なる問題、狭幅で ref バッジが件名を押し出す問題も修正
- 実機（/Applications v0.5.11）で幅 220 / 325 / 700pt のスクショと stage / unstage /
  一括ステージ / commit 両経路 / push / pull / 連打 / 異常系を実クリック検証。
  証拠は ~/dev/tako-evidence/494/

## 2026-07-25（#497: IME 未確定文字列がカーソル非表示ペインで消える問題を修正）
- 下線オーバーレイの anchor が `pane_cursor_origin`（CursorShape::Hidden で None）を
  使っていたのが原因。#29 が候補ウィンドウ側だけ `ime_cursor` フォールバックへ移した
  取りこぼし。`ime_overlay_anchor()` を新設し render と回帰テストが同じ経路を通る形に
- tako-core に `Screen::ime_anchor_cell()` を新設して不変条件を API に明示 + 単体テスト
- セルフテスト 76c（カーソル非表示ペイン）/ 76d（split 直後）を追加。修正を戻すと 76c が
  FAILED になることを実測して検出力を確認。実機でも下線が出ることをスクショで確認

## 2026-07-25（#501: セルフテストが 33b で中断し以降の全項目が未実行だった問題を修正）
- テーマ適用が「保存 → ディスク読み直し」順で、セルフテストは保存だけスキップするため
  適用が即座に巻き戻っていた。33b で exit(1) するので 75 / 76 / 77 等が一切走っておらず、
  #497 の回帰を検出できなかった一因。保存しないときはメモリ上の適用を正とするよう修正
- 隔離セルフテストが通しで完走（TAKO_APP_SELF_TEST_OK、FAILED 0）


## 2026-07-25（#500 Part 1-4 + #504: プロファイル env 注入 + アカウントレジストリ）
- #500: Profile に env マップ追加、master/worker 全経路に注入（export 後勝ち方式で direnv に勝つ）、内部変数拒否、値マスク、projects 制限強制、CLI/MCP 1:1、起動時可視化
- #504: accounts.yaml CRUD（116 ツール）、spawn の account パラメータ、master_account/worker_account、model/effort 解決順（spawn > アカウント > プロファイル）
- 関連コミット: `7c97611`（#500）、`28b0eca`（#504）→ PR #505
- 次: レビュー → squash merge → 隔離実測 → Part 5-7 は別タスク

## 2026-07-25（#500 Part 5-7: cwd + ファイルツリー自動追加 + 専任マスター）
- Part 5: Profile に cwd。master 起動時に cd + 存在検証 + ~ 展開。インライン / --tab 両対応
- Part 6: master 起動後に cwd + projects のフォルダをファイルツリーへ IPC 経由で自動追加
- Part 7: projects 付きプロファイルで system prompt に Assigned Projects 注入。未登録 key は起動時エラー
- 関連コミット: `81b8bea` → PR #506（Closes #500）
- 次: レビュー → squash merge → install

## 2026-07-25（#503: テキスト入力フラグ残留でキー入力が奪われる問題を根治）
- `clear_text_input_focus()` 新設。9 経路（タブ切替 / フォーカス移動 / パネル非表示 / CLI dispatch 等）にクリア配置 + handle_key の防御的クリア。セルフテスト項目 81 追加
- 関連コミット: `9dc9c08` → PR #509（Closes #503）

## 2026-07-25（#495: git タブのコミット詳細表示）
- コミットクリックで変更ファイル一覧 + diff 展開。`git show` の CLI 出力空応答も修正
- 関連コミット: `fc6c55c` `0bc9c4f` → PR #507（Closes #495）

## 2026-07-25（#498: stale claude バイナリの検知と張り直し）
- 起動時に PATH 上の claude バイナリの実在・実行可能性を検証し、stale なら再検出
- 関連コミット: `ab32ff7` → PR #508（Closes #498）

## 2026-07-25（#495 UX 改善: 詳細ビューを選択カード直下へ + CLI 空応答修正）
- 詳細ビューの位置を 200 件末尾から選択カード直下へ移動。`render_commit_detail()` メソッド化。CLI `print_result` の `GitCommand::Show` 未登録を修正（`0bc9c4f`）
- 関連コミット: `86b7769` `0bc9c4f` → PR #510（Closes #495）

## 2026-07-25（#467: Windows 移植の実装フェーズ開始 — 設計 + Issue 分割 + P0）
- ポーティングアーキテクチャ設計を新設（`.agent/plans/2026-07-windows-port-architecture.md`）:
  抽象境界 B1〜B15 / 機械可読サポートマトリクス + パリティテスト T1〜T6 / prompt の単一ソース化 /
  mac 先行 → Windows 一括反映のワークフロー
- 実装 Issue 15 件を起票（#514〜#528。プレ版 v0 = タブ・ペイン管理 → 永続 → git タブ → プレビュー順）。
  #513（設定の git 共有）も依存関係つきでツリーに組み込み
- P0 完了: 呼び出し側に cfg を足さず B1/B4/B5/B8 を新設して解決。macOS から
  `cargo check --workspace --target x86_64-pc-windows-msvc` がエラーゼロ（`scripts/check-windows.sh`）。
  `.agent/windows-setup.md` で実機初回ビルド手順を用意
- 関連: PR #529（`Refs #467, #514`）。1268 tests / fmt / clippy 全緑
- 次: Windows 実機で `cargo build` → #517（プレ版 v0）着手

## 2026-07-25（#515: プラットフォーム抽象基盤 — 対応マトリクスとパリティテスト）
- `tako-core::platform::support` 新設: 119 機能 × macOS / Windows の機械可読マトリクス。
  判定は純粋関数で **macOS 上から Windows 側の縮退表を検証できる**。理由文は `Note`（日英対）で
  1 箇所定義し、UI・診断・prompt が全部そこから引く（#435 準拠）
- パリティテスト: T1 被覆 / T2 逆被覆 / T3 CLI 表（179 リーフコマンド）/ T4 説明必須 + 日英検査 /
  T5 診断一致 / T6 単一ソース + スナップショット同期。**全部 macOS の `cargo test` で回る**
- `tako platform` + MCP `tako_platform`（計 119 ツール）。CLI は GUI 不要のローカル処理
- 副産物: MCP スナップショットが `tako_git_show`（#495）/ `tako_stale_binary`（#498）を欠いており
  セルフテスト項目 32 が main で失敗する状態だったのを発見・修正
- 関連: PR（`Refs #467, #515`）。1287 tests / fmt / clippy / クロス check 全緑
- 次: #516（単一ソース化）・#517（プレ版 v0）

## 2026-07-26（#516: system prompt / setup 配布物の単一ソース化）
- `tako-control::platform::facts` 新設。正本 3 本（master / solo / setup）に `{{platform_notes}}` を
  1 個だけ置き、レンダリング時に注入。縮退理由はマトリクス（#515）から自動生成するので
  prompt 側の記述更新は不要
- `changes.yaml` に `platforms:`（省略 = 全プラットフォーム。未知の値はパースで弾く）
- setup の `SYSTEM_PROMPT` を tako-cli から tako-control へ移し正本を一元化
- 実装中に実バグを検出: `PlatformFacts` が理由文を生成時の言語で凍結していた
  → `Note` のまま保持し描画時に解決する形へ修正（#518 に警告したのと同じ罠）
- 関連: PR（`Refs #467, #516`）。1298 tests / fmt / clippy / クロス check 全緑

## 2026-07-26（#520: git タブ Windows 対応の macOS 完結部分）
- `git::to_git_path` / `repo_relative` / `from_git_path` / `normalize_repo_root` を新設。
  git は常に `/` 区切りでパスを出し入れするので、`strip_prefix` の結果をそのまま渡すと
  Windows で `src\foo.rs` になり履歴が空で返る（実バグ）。呼び出し 2 箇所を境界経由へ
- CRLF 耐性テストを追加（parse_log / parse_branches / parse_status / parse_diff）。
  `str::lines()` が `\r` を落とす前提に依存しているので退行検出用に固定
- UI（right_panel.rs）は #496 が大改修中のため未着手。#520 にコメントで積む
- 関連: PR（`Refs #467, #520`）。1306 tests / fmt / clippy / クロス check 全緑
## 2026-07-25（#518 設計 + #519 段取り①②: 永続バックエンド境界 B2 の新設）
- #518 設計（`.agent/plans/2026-07-windows-persistence-backend.md`、PR #531 merge 済み）:
  tmux の役割を「生存の器」と「アウトオブプロセス到達」に分離。後者はほぼ全経路で
  フォールバックであることを実コードで確認し、trait を 2 段（`SessionBackend` / `DetachedAccess`）にした
- #519 ①②: `tako-core::backend` 新設（`SessionRef` newtype で #428 のターゲット式取り違えを構造排除、
  `TmuxBackend` は既存自由関数への委譲、`NullBackend` は器なし）。`TAKO_BACKEND=none|tmux|auto` で
  **macOS 上に Windows の縮退経路が生えた**（`available()` を「tmux が選択されたか」に一本化）
- 実測（隔離）: none = layout.json 保存あり / session null / tmux サーバー不在 / 復元は構成と cwd のみ・
  画面マーカー 2→0、auto = 再 attach 2・マーカー 2→1。セルフテスト（tmux モード）は FAILED 0 で完走
- 関連: PR（`Refs #519`）。1280 tests / fmt / clippy / check-windows.sh 全緑
- 次: 段取り③（`PaneReach` 導入・dispatch 約 40 箇所）→ ④⑤⑥
## 2026-07-26（#496: git タブのブランチ操作 + コンフリクト解消エージェント）
- ブランチ切替 / 作成 / マージを UI・CLI・MCP へ 1:1 追加。破壊的操作は confirm の
  dry-run 方式に統一し、切替は「持ち越し」と「妨げる変更」を分けて提示、マージは
  `git merge-tree --write-tree` で作業ツリーに触れずコンフリクトを事前予測する
- コンフリクトカード（進行中操作 / ours←theirs / 未解決ファイル / 中止）と
  claude・codex・agy から選ぶ解消エージェント起動を新設。プロンプトは埋め込み雛形 +
  `<data_dir>/orchestrator/conflict-resolver.md` で差し替え可
- 関連コミット: `c22990f` `5202033` `b9430cb`（+ `150ec3b` i18n テストのフレーク修正）→ PR #534
- 検証: 品質ゲート全緑（1281）+ 隔離セルフテスト完走 + CLI 13 手順 / MCP 7 手順 +
  実クリック 4 件（証拠 ~/dev/tako-evidence/496/）。カード類のスクショは蓋閉じで未取得
- 次: 蓋を開けた状態でコンフリクトカード・狭幅のスクショを取得 → master 検収 → merge

## 2026-07-26（#522: OS 連携（B8）の集約 — open / osascript を境界の内側へ）
- `platform::os_integration` に `open_url` / `open_url_wait` / `open_in_text_editor` /
  `open_new_instance` / `pick_application` / `move_to_trash` / `notify` を追加し、
  散在していた `open` 14 箇所・`osascript` 3 箇所を全部そこへ寄せた（挙動不変）。
  呼び出し側の `cfg` は **11 個減り**、境界の内側だけが 5 個増えた
- 権限昇格（`osascript … with administrator privileges`）は B8 に入れず B9（`sleep_guard`）の
  内側に留めた。汎用の昇格 API を OS 連携境界に置くと危険な踏み台になるため
- 受け入れ条件 2 の番犬テスト `os連携の直呼びが境界の外に残っていない` を新設。
  意図的な違反を注入して落ちることまで確認（検出力の実証）
- 検証: 1313 tests / fmt / clippy / クロス check（警告 13 = baseline 不変）/ 隔離セルフテスト
  FAILED 0 / ゴミ箱移動の実 e2e（#80 インジェクション検証）緑
- 次: #522 の残り（links.rs の Windows パス・Windows 実機の手動チェック・IME）

## 2026-07-26（#519 段取り③: PaneReach 導入 — 到達手段を型で表す）
- `tako-control::reach` 新設。`PaneReach { InProcess | Detached | Unreachable }` の網羅 match と
  `UnreachableReason`（NoSession / NoDetachedAccess / InvalidSession）で、
  「フォールバックが失敗した」と「そもそも到達手段が無い」を初めて型で区別した
- dispatch.rs の tmux 直呼びを **39 → 17 箇所**（22 箇所を境界へ）。Send / Read のフォールバック、
  レジストリ列挙、report 第 1 層、worker_status の 4 経路、permission ダイアログの採取と応答
- `DetachedAccess::capture_history_joined` を追加（`-J` あり = 報告用。`capture_history` とは別物）
- 残る 17 = `tako_tmux_*` の機能面そのもの（任意 tmux サーバー操作。Windows は Pending）+
  `available()` 2 箇所（段取り④で capabilities へ）
- 実測: `TAKO_BACKEND=none` で `read` / `send` が「永続バックエンド（none）に…到達手段が無い」の
  構造化エラーを返し、`auto` では従来どおり `can't find session`（挙動不変）
- 次: 段取り④（persist ゲートの言い換え）→ ⑤⑥

## 2026-07-26（#519 段取り④: persist ゲートを capabilities へ言い換え）
- 呼び出し側の問いを「tmux があるか」から「backend に何ができるか」へ変え、
  `tmux_backend::available()` を**全廃**（本体ごと削除。再導入を構造的に防ぐ）。
  器を問う 9 箇所は `capabilities().survives_app_exit`、remote は `detached_access` へ
  （remote デーモンは tako-app とは別プロセスなので、必要なのは器ではなく到達手段）
- `BackendCapabilities::degraded_note()` / `describe()` を新設。縮退の説明を 1 箇所で定義し、
  `tako persist` / MCP が `backend`（label / survives_app_exit / detached_access / scrollback / note）
  を返すようにした。`available` は後方互換で存置
- 実測: `tako persist` が none = `note` つきで全 false、auto = `note: null` で全 true。
  縮退経路・セルフテスト・不変条件は ③ と同一の結果
- 挙動差の申告: remote の起動拒否メッセージを「tmux が見つからない」から能力ベースの文面へ
- 次: 段取り⑤（WorkerEntry.pid / report の pane_log フォールバック / delivery 表示）→ ⑥

## 2026-07-27（Windows 永続化 M1: wrap_spawn の本番配線 — Windows 実機で作業）
- 本番の spawn 経路が `tmux_backend::wrap_options` を直呼びしており、案 B-1（ConPTY
  セッションホスト）を登録しても器が一度も使われない状態だった。役割 A（生成・破棄・
  列挙・環境・tty）13 箇所を `SessionBackend` 経由へ寄せ、器の割り当てと spawn の
  書き換えを `backend::wrap_spawn_for_pane` の 1 箇所に集約（設計 §3.6 の合格条件を充足）
- 番犬テスト `器のライフサイクルの直呼びが境界の外に残っていない` を新設（違反注入で
  FAILED を確認）。#522 の番犬が Windows で常に落ちていた原因（許可リストのパス比較が
  `\` と `/` で不一致。#520 と同種）も共有スキャナ側で修正
- 実測（隔離・Windows 実機・backend=none）: split / 明示コマンド / 復元の 3 経路で
  ペイン生成 → シェル実動作、`session: null` の構造のみ永続化、強制終了 → 再起動で
  1 タブ 2 ペイン + cwd 復元（再 attach 0 / 新規シェル 2）、persist OFF/ON の往復も通過
- M0（ConPTY 生存検証）の使い捨てコードを `poc/conpty-survival/` として取り込み（罠 7 項目）
- 関連コミット: `984de32`（境界）`7d4be2b`（本番配線）`9a995b4`（M0 PoC）。**push 禁止の
  Windows 検証機のためローカルブランチ `windows/467-ipc-orchestration-local` に積むのみ**
- 次: M2（session-host 本体）。触るのは `backend/` の中だけでよい状態になっている

## 2026-07-27（#587: Windows インストーラー（Inno Setup）+ リリースワークフロー）
- per-user インストーラー（PATH 追加・重複防止・アンインストールで復元）+ .ico 生成 +
  ポータブル zip + `windows-release.yml` を新設。実機で install → `tako --version` →
  GUI 起動 → 再インストール → uninstall → PATH 完全復元まで通し実測
- MSVC CRT を静的リンク（`.cargo/config.toml`）。VCRUNTIME140* の import が消えることと、
  静的/動的で全テスト結果が同一（既知の POSIX 前提 19 件のみ、#583 相当）であることを A/B 実測
- 関連コミット: `9a8dbef`（CRT 静的）`9059402`（インストーラー）。**push は master 側**。
  ワークフローは未実行、`tako-app.exe` へのアイコン埋め込みは別タスク

## 2026-07-27（#587: Windows リリースを GitHub Actions からローカル実行へ転換）
- `windows-release.yml` を未実行のまま削除し、実機で「前検査 → build-installer.ps1 →
  配布物のスモーク検査 → gh release upload」を通す `release-windows.ps1` を新設。
  既定は dry-run で、`-Force` が緩めるのは git の状態だけ（版数整合と配布物検査は緩めない）
- dry-run 実測で実バグ 1 件を検出・修正: Inno Setup の setup exe は FileVersion を
  空白詰め（`"0.5.12              "`）で返すため、成分比較の前に空白を落とす必要がある
- 関連コミット: 経緯は `74ad133`。**中身は並行作業のコミット `1ab42de` / `845966f` に
  巻き込まれて入っている**（push 済み PR #588 上のため履歴は書き換えていない）
- 次: 実アップロード（`-Upload`）は master 側で実施

## 2026-07-27（#528: Windows の更新通知が「押すと必ず失敗する」問題を根治）
- 通知判定を配布アセット基準に変更。Windows は `tako-setup-{tag}-x64.exe` が実在する
  リリースだけを「更新あり」とし、macOS だけの夜間リリースではバナーを出さない
  （出しても押せば必ず失敗するため）。更新実行は setup exe を DL → サイズ + PE ヘッダで
  整合確認 → `/SILENT /NORESTART` 起動 → アプリ終了（実行中 exe は自分で置き換えられない）
- 分岐は `UpdateTarget`（`cfg!` は 1 箇所）に閉じ、判定は純粋関数化して **macOS 上からでも
  Windows 側の挙動をテストできる**形にした（#515 と同じ方針）。macOS 経路は不変
- 次: Windows アセット付き Release が出たら実機で通知 → 更新の通し確認（未検証）

## 2026-07-27（#519 M2: Windows 永続化の器を psmux で実装）
- 自作せず既存 OSS の psmux（MIT / Rust / tmux 互換 CLI）を器に採用し、設計 §2.2 の案 B-1
  （器あり・到達なし）として `PsmuxBackend` を新設。`Choice` へ 1 つ足すだけで器が生え、
  呼び出し側の変更は #177 の強奪ガード 1 箇所のみ（M1 の合格条件どおり）
- TmuxBackend は流用不可（`kill-session -t =name` が 3/3 失敗・各 5.1 秒ブロック）。
  `=` 不使用・`show-environment` の全変数出力・`history_bytes` 非依存・`pane_tty=None`・
  conf の `warm off`・未検証バージョンの起動時プローブ、の 6 点で非互換を吸収
- 多重起動安全性は器に尋ねられない（`list-clients -F` 無視 + `-D` が効かない + Windows では
  `is_live_tako_app` が常に false）ため、OS ファイルロックを生存の証明に使う
  オーナー記録 `backend/owner.rs` を新設。2 個目の tako がセカンダリへ降格することを実測
- tmux 誤判別ガード: psmux は `tmux.exe` を配り `-V` の 1 行目で tmux を詐称する。
  2 行目で判別し、Windows では POSIX 版 tmux も器に選ばない
- 実測: kill 前 tick 6 → tako 不在中 21 → 再起動後 41、pane_pid 同一、
  persist.log「tmux 再 attach 1 / 新規シェル 0」。単体 23 + 実バイナリ統合 10 本
- 関連コミット: `cf6ee4a` `7b9b74c` `1ab42de` `501793a` `845966f` `8778d14` `5ed0859`
- 次: psmux の導入経路（同梱するか）の判断。役割 B（到達）は引き続き無い

## 2026-07-27（#602: Windows のペイン/タブ操作ショートカットを完了）
- #585 の 45 本は移植済みで分割は Ctrl+Shift+D で動くと確認（ユーザーの「効かない」は旧バイナリ由来）。
  Ctrl+D は EOF(0x04) と衝突するため分割に採らず、判断をテストで固定
- `ctrl-shift-q` → Quit 追加（Windows に正規の終了手段が無かった）+ `cmd-h`/`cmd-alt-h`/`cmd-m` を
  macOS 限定へ（Win+Alt+H が届くと `hide_other_apps()` の `unimplemented!()` で app ごと abort）
- 関連コミット: `f69c6ff`。220 passed / 0 failed（変更前 217）。副産物 #603（✕/Alt+F4 のゾンビ化）起票

## 2026-07-27（#582 続報: 候補ウィンドウが入力行に被る問題を CFS_EXCLUDE で根治）
- IMM32 の読み戻しでアンカーは正しい（セル下端ちょうど）と確定。真因は GPUI が
  `CANDIDATEFORM` に**点だけ**（`CFS_CANDIDATEPOS`）を渡すこと。画面下端に余白が無いと
  IME はその点に候補ウィンドウの**下端**を合わせて反転し、入力行を覆う（= 最大化時の常態）
- B17 に `set_candidate_exclusion()`（`CFS_EXCLUDE` + 未確定文字列の矩形）を追加。基準点は不変
- 実測（1920x1080 @ 125%・入力行セル 物理 y=930..951）: 候補ウィンドウ 662..948（覆う）→
  642..928（覆わない）。余白があるときは 335..621 → 312..598 で入力行の直下に密着

## 2026-07-27（#610: ターミナルのディセンダー欠けを根治 — 行高の継承漏れ）
- 真因は GPUI の `StyledText` が**行高とフォントサイズを `window.text_style()`（継承側）から取る**こと。
  `with_default_highlights` に渡す TextStyle は TextRun にしか効かず、tako は行高を継承側で
  一度も指定していなかったため既定の `phi()`（1.618×13px ≒ 21px）が使われ、17px の行 div +
  `overflow_hidden` の外へベースラインが約 2px 落ちて g/q/y/p の下部が切れていた
- 修正は行 div に `.text_size(px(font_px))` + `.line_height(px(line_h))` の 2 行のみ。
  セル寸法・クリック判定・IME アンカーは不変（行ボックス内のベースライン位置だけが変わる）
- 実測（125% DPI）: `q` の ink 10 → 12 物理行（あるべき 12.2）。副次的に SGR 4 の下線
  （丸ごと消えていた）とペイン単位フォント拡大（グリフが 13px のままだった）も回復
- 検証: fmt / clippy（baseline と完全一致・新規 0）/ tako-app 220 passed 0 failed +
  隔離 GUI の before/after ピクセル実測。macOS は未コンパイル・未目視（mac 側で要確認）

## 2026-07-27（#525: tako setup の Windows パリティ — 検出全滅の根治）
- 実測でマトリクスの誤りを確定: `tako_setup` = Degraded「設定の生成は動く」は嘘で、
  Windows では **1 ステップ目で exit 1**。`find_command()` が `$SHELL -l -c "command -v"`
  直呼びで、`$SHELL` 不在 → `/bin/sh` spawn 失敗 → claude / git が導入済みでも全滅していた
- 抽象境界 **B16（`platform::exe`）** を新設して根治（unix は従来のログインシェル経由を維持、
  Windows は PATH + PATHEXT + ユーザー導入先。探索の中核は純粋関数で mac からも検証可）。
  同根の `dispatch::which` / `which_claude`（`which` 直呼び）も寄せて MCP 自動登録が成立
- setup 側の mac 前提を一掃: 依存表を psmux / git へ分離・案内を winget 化・配布物を
  `paths::data_dir()` へ・`date` 子プロセス廃止（`completed_at` が "unknown" だった）・
  スリープ防止と remote 案内をマトリクスで gate・シェル統合の状態と理由を明示・
  MCP 登録失敗で setup 全体を止めず手動手順を提示
- 検証: 全 1416 passed（失敗 13 件は既知の POSIX 前提のみ・baseline と同一）/ clippy 19 件も
  baseline と同一 / fmt 緑 / 実機 e2e 4 種（初回・2 回目冪等・--check・CLI 不在）
- 残: PowerShell の OSC 7/133 統合本体は未着手（#525 に残す）。Windows のロケール検出は #604

## 2026-07-27（#604: Windows で language:"system" が常に英語になる問題を根治）
- 抽象境界 **B10（`platform::locale::system_languages()`）** を新設。macOS の AppleLanguages
  経路を境界の内側へ移し、Windows は `GetUserPreferredUILanguages`（**表示言語**。
  `GetUserDefaultLocaleName` は「地域形式」で別物）を追加。`detect_os_lang` から `cfg` が消えた
- 実測（表示言語 ja-JP の実機・環境変数なし）: 修正前 `System.resolve() = En` → 修正後 `Ja`。
  `system_languages() = ["ja-JP", "en-US"]`。`TAKO_LANG` 優先は不変（テストで固定）
- 検証: tako-core 460 passed（HEAD と同一の既知 6 失敗のみ・新規テスト 12 本）/ tako-app 220 全緑 /
  parity 11 全緑 / doc 再生成の差分なし。macOS は cfg 構造保証のみ（未コンパイル）

## 2026-07-27（#623: Windows の IME 打鍵消失を調査 — 潜在欠陥を 1 件修正、症状の根治は未確認）
- GPUI Windows は描画のたび `update_ime_enabled()` を呼び、入力ハンドラがスロットに無いだけで
  `CPS_COMPLETE`（未確定文字列の強制確定）+ IME 切り離しを行う。ハンドラが戻るのは paint で
  `handle_input` が呼ばれたときだけ = フォーカスが自分のハンドルにある時だけ。既存の自己修復は
  「どこにもフォーカスが無い」しか見ておらず、`on_focus_lost` は同フレームの paint に間に合わない
  → canvas の **prepaint** で取り戻す + 切り離しを検知したら再結合（B17 に `is_associated` /
  `reassociate` / `guard_action`）+ `render` 冒頭の判定を「自分のハンドルに無い」へ強化
- **ただし実測ではこの経路は一度も発火せず**（`focus_held=true associated=Some(true)` のまま。
  OS フォーカスを 5 往復奪っても不変）。合成 IME 入力での再現も、ハーネスを直したら 7/7 正常。
  症状は「未確定文字列が壊れる」ではなく**打鍵が IME へ届く前に落ちる / 重複する**線が濃い
  （報告 3 例すべてその仮説で説明がつく）。切り分け用に `TAKO_IME_DIAG=1` の診断を新設
- 次: 実機で症状再現時に `TAKO_IME_DIAG=1` のログを採取 → #623 §4（`translate_accelerator` が
  処理済み打鍵の `TranslateMessage`/`DispatchMessageW` を飛ばす経路）の検証

## 2026-07-27（#623 続き: VK_PROCESSKEY 説を否定 + PTY 書き込み欠落の切り分け）
- **VK_PROCESSKEY 経路は否定**（コード変更なし・doc に記録）。`translate_accelerator` が
  「アプリが処理済み」の打鍵で `TranslateMessage`/`DispatchMessageW` を飛ばすのは事実だが、
  IME が食う打鍵（wParam=0xE5）は `MapVirtualKeyW(0xE5, MAPVK_VK_TO_CHAR)` が **実測 0** のため
  `parse_normal_key` が None → 「未処理」で返り、素通りして IME へ届く。実 IME 変換中に
  `handle_key` の診断が出るのは Esc のときだけ、という実測とも一致
- **PTY 書き込みの欠落は未確定**。直接 spawn（persist OFF）は 600/2000/8000 バイトがバイト一致、
  psmux 本番ペインも**行が綺麗なら** 968 バイトまで 8/8 一致。長い掃引が NG に見えたのは
  ①存在しない `--enter` を渡して CLI がエラー終了 ②失敗行が残ったまま次を送って連鎖、の
  ハーネス由来。ユーザー実例（claude ペイン = alt screen の bracketed paste 経路）は未計測
- 次: alt screen ペインでの `queue_send_flow`（paste + 分離 Enter）経路をバイト単位で測る

## 2026-07-28（#628: 使用中のコンソール窓チラつきを根治 — tmux 経路の抑止漏れ）
- 真因は `tako_core::tmux::tmux_command()`（tmux / psmux を叩く全経路の合流点）に
  `no_console_window` が無かったこと。#586 で境界は作ったが被覆が 96 中 16 箇所に留まり、
  ポーリングの度に psmux の `tmux.exe` が新しいコンソールを持って明滅していた
- 実測（30 秒・修正前）: tmux/psmux を親とする conhost 生成 22 個 + tako-app 直下 6 個
- 棚卸しで他 11 箇所も境界経由へ寄せ（被覆 16→33）、番犬テスト
  `コンソール窓を抑止していない子プロセス起動が増えていない` を新設（残り 63 箇所を理由つき件数固定）

## 2026-07-28（#623: 長文プロンプトの送達で Enter を早撃ち・連打していた問題を根治）
- 計測用の器（alt screen + bracketed paste を要求し、受信バイトを時刻つきで記録する
  偽 claude TUI）を作り、`TAKO_FLOW_DIAG=1` の分岐ログと突き合わせて実証。
  **仮説「画面テキスト一致の誤判定 → 早すぎ Enter / 再送」は当たり**だった
- 実測（本文 2164 バイト・15 行・描画 3 秒遅延の器）: 入力欄が **78 文字**しか
  描画されていない時点で「反映済み」と判定して Enter を撃ち、さらに送信後も
  描画が追いつかず「残留」と誤判定して Enter を **4 回**撃ち直していた（空送信 4 件）
- 対処 = ①入力欄の長さが据え置きになるまで Enter を待つ ②送信後に入力欄が Enter 直前と
  完全一致なら「未送信」と「未描画」を区別できないので画面が動くまで再送を保留
  （10 秒動かなければ従来どおり未送信扱い）。修正後は遅延あり / なしとも送信 1 件のみ、
  本文は完全一致。tako-app 224 passed（新規 4 本）、tako-control の失敗 11 件は baseline と同一
- 計測で分かった罠: ①`ENABLE_VIRTUAL_TERMINAL_INPUT` を立てない器には ConPTY が
  VT をキーイベントへ翻訳して渡すため bracketed paste の括りが届かない
  ②コンソール入力の既定コードページ（CP932）だとバイト比較が成立しない

## 2026-07-29（#642: IME 未確定文字列のプレビューが本来の行より約 2px 下にずれる問題を根治）
- 真因は #610 の取りこぼし。GPUI の `StyledText` は**行ボックスをフォントサイズとともに
  `window.text_style()`（継承側）から取る**ため、継承側に行高を書かないと既定の `phi()`
  （1.618034 × 13px ≒ 21px）が使われ、ベースラインが `(行高 - ascent - descent) / 2 + ascent`
  へ置かれて 17px の箱より `(21-17)/2 ≒ 2` 論理px 下がる。#610 で行 div 側だけ直したので、
  IME オーバーレイだけが取り残されて**相対ずれとして顕在化**した（それ以前は本文も同じだけ
  ずれていたので見えなかった）
- 修正は該当 div に `.text_size()` / `.line_height()` を明示し、箱・文字・TextStyle を
  **IME 対象ペイン**の値（`pane_font_size` / `pane_line_height` / `pane_text_style`）へ統一。
  アンカーは元からペイン単位なので、Ctrl+= したペインでの行単位のずれも同時に解消する
- ずれ量は 100% = 物理 2.0px / **125% = 2.5px** / 150% = 3.5px（`pixel_snap` は
  `f32::round()` ではなく half-toward-zero なので 150% の 25.5 は 25 へ落ちる）。
  単体テスト 4 本（DPI 表 + 修正後 0 + 番犬）を追加。tako-app 228 passed 0 failed
- 実機目視は未検証（人手確認待ち）。候補ウィンドウ側（#582 の `CFS_EXCLUDE`）は不変

## 2026-07-29（#643: 「ちらつき・描画が不安定」を実測で 2 原因に切り分け）
- ちらつきは #628 のコンソール窓で確定。稼働バイナリ（0.5.13・07-28 01:37）が修正
  `88f91ac`（02:07）より 30 分古く、毎秒 1.5 個の conhost が明滅していた（修正入りの
  隔離起動では 45 秒で 1 個 = 起動時 ConPTY のみ）。**コード修正は不要・版の入れ替えだけ**
- 「描画が不安定」= perf.log の「UI ストール」の誤認。foreground executor の再開遅延は
  Windows では WinRT スレッドプール + メインスレッドキューの 2 段で、どちらの遅れか
  区別できていなかった（98 件中 94 件が perf_span と非共起。#168 では 1021 件全件共起）。
  watchdog の素の OS スレッドの sleep 超過を物差しに `classify_stall` で 3 分類へ
- 端末イベントを 1 件 = 1 往復で `update` していた経路をまとめ処理化（上限 256・
  `TAKO_TERM_EVENT_DRAIN=0` で旧挙動）+ `term_events` スパン新設（従来は計測の外）
- アイドル計測で「常時 dirty で毎 vsync 描画」説は否定（10 秒窓 5 本中 render 8 回）

## 2026-07-30（#524: Windows のポート検知とスリープ防止を実装）
- B5 検査側: `GetExtendedTcpTable`（v4/v6）+ Toolhelp32 で「PTY 直下プロセスの子孫」から
  listen ポートを検知（`platform::procinfo` 新設）。ペインのキーは OS で実体が違うので
  `ports::pane_key` に閉じ込め、呼び出し側は不透明 u64 のまま単一経路
- B9: `PowerCreateRequest` + `PowerSetRequest` でアイドルスリープを抑止（`platform::power` 新設）。
  蓋閉じ / sudoers / thermal は macOS 固有 capability として `lid_control_supported()` で分離
- 実機検証で `POWER_REQUEST_TYPE` の値取り違え（3 = ExecutionRequired）を検出・修正。
  `powercfg /requests` の SYSTEM 欄に出ることまで確認。マトリクスは port_detect = supported /
  sleep_guard = degraded（蓋閉じ非対応の理由つき）
- 罠: 並行 worker と `CARGO_TARGET_DIR` を共有すると同一ハッシュの rlib が上書きされ、
  **別ブランチの成果物が link される**（baseline のまま動く）。検証は worktree 専用 target dir で行う


## 2026-07-30（#640: spawn の起動コマンドが新規ペインに届かない問題を根治）
- 真因は「PTY を起こした直後に本文 + Enter を書きっぱなしにしていた」こと。器（psmux）が
  入力を読み始める前のバイトは落ちる。実測: 0〜500ms は**全損**、1500〜3000ms は**中抜け**
  （`WriteOutput TKOMARK640` = 'A' が消える）、6000ms 以降は全到達。器を挟まない直接 spawn は
  20/20 全到達 = **落としているのは器の側**。pwsh のプロファイル読込は無関係（差 50ms）
- 報告された 4 症状（完全消失 / 大幅遅延 / 文字単位ドロップ / Enter だけ欠落）は、この欠落 1 つで
  説明がつく。中抜けするとシェルが継続プロンプト `>>` で待つため、後から届いた入力が
  くっついて遅れて発火する
- `tako_core::shell_send`（純粋ステートマシン）を新設し、起動コマンドを流す 4 経路
  （spawn / handoff / sessions resume / git resolve）を送達確認つきへ寄せた。
  **Enter は本文が全文エコーで見えているときしか撃たない**
- 実装で踏んだ罠 2 つ: ①経過時間で打ち切ると、高負荷時（エコーが 1 秒に数文字）に
  届いている最中へ書き直して行が混ざる → 「進みが止まったとき」だけ壊れたと判断する
  ②Ctrl+C で捨てた `… ^C` の行を自分のエコーと取り違える → 出現数の増加で見る
- 実測: 旧経路 0/15 到達 → 新経路 15/15 到達。macOS は CI（build + lint + test）緑・目視は未実施
- 関連コミット: PR #670。次: 実機ビルドで spawn 成功の確認（未検証）

## 2026-07-30（#657: Windows に in-window メニューバー行を新設）
- タブバーの一段上に 30px の自前メニューバー行（ファイル / 編集 / 表示 / ウインドウ / ヘルプ）を
  足し、#584 のウィンドウコントロールをその行の右端へ移設（`menu_bar.rs` 新設）。構成の正は
  `app_menus()` 1 箇所で、Windows 版は `HideOthers` / `ShowAllApps`（`gpui_windows` で
  `unimplemented!()` = abort）を構造的に除外し番犬テストで固定
- `TAB_BAR_HEIGHT` の直参照を `top_chrome_height()` へ集約（行を足すとペイン矩形・IME
  アンカー・境界ドラッグの座標が全部ずれるため）。CLI `tako menu` + MCP `tako_menu` を 1:1 追加
- **見た目と操作感は実機未検証**（稼働中の tako を終了できない制約。次のプレビュービルドで
  ユーザー確認）。上端 8px 前後が OS のリサイズエッジに食われる件はトリガーを下げて回避済み


## 2026-07-30（#662: master が MCP で worker の対話ダイアログを操作できるようにする）
- `tako_orchestrator_dialog`（内容取得）+ `tako_orchestrator_respond --answers`（複数質問 /
  multiSelect / dry_run）+ `tako_send_keys` / `tako keys`（生キー送出）を新設。計 128 ツール
- respond が Windows で必ず死んでいた根因 = 到達手段を detached 固定で解決していたこと。
  in-process 優先の `DialogReach` を新設して psmux（detached_access:false）でも動く形に
- **設計変更**: claude はダイアログ表示中 transcript に何も書かない（`tool_use` は回答確定後に
  `tool_result` と同時出現）と隔離 E2E で実測 → 保留中の正はライブ画面へ。副産物として
  `find_transcript` が CLAUDE_CONFIG_DIR / アカウント別 config dir を見ていない実バグも修正
- 関連: PR #679。証拠は #662 のコメント（内容取得 / dry_run / 本送信 / worker 受領 /
  矢印キーのハイライト前後比較 / 誤爆防止）

## 2026-07-30（#665: orchestrator の運用保証 — spawn の起動保証 + 再アーム不要の常時監視）
- spawn の投げっぱなしを段階検証つきの状態機械へ（queued → shell_ready → launch_sent →
  agent_started → prompt_sent → prompt_delivered、確認できなければ再送）。画面分類は
  tako-control の純粋関数に置き GUI なしで検証可。#640 とは「シェルに届いたか」/
  「エージェントが起動したか」で役割分担し、送出は `queue_command_flow` へ委ねる
- #401 の supervisor は**呼び出し元ゼロ**（自動復旧は一度も動いていなかった）→ 多 worker を
  1 本で見る非ブロッキング版へ置き換えて配線。毎周期レジストリを読み直すので master は
  再アームしない。イベントは `supervisor-events.jsonl` + cursor でプロセス跨ぎ配送
- 関連: PR #682（Closes #665、#663 の 1・2 も消化）。隔離 E2E で実 claude を含む 7 ケース実測。
  E2E 中に実バグ 3 件（PowerShell 7 の文言違い / 空行で窓が埋まりエラー見逃し / 常駐の残留）を発見・修正

## 2026-07-30（#654: Windows でホイールスクロールが全ペインで効かない問題を根治）
- 真因は `mirror_scroll_pane` が器の `ScrollbackAuthority` を見ず、`InProcess` を申告している
  psmux ペインまでミラー経路へ載せていたこと。psmux は `#{mouse_any_flag}` / `send-keys -H` を
  持たない（実測）ので `history_state` が None を返し続け、ホイール・スクロールバー・CLI Scroll が
  全ペインで無反応になっていた。判定を純粋関数化して器の申告を通し、直接ペイン経路へ落とす
- ホイール量も是正: Windows の `ScrollDelta::Lines` は gpui が `SPI_GETWHEELSCROLLLINES` を
  掛けた値なので、tako 側の ×3 で 1 ノッチ = 9 行になっていた（倍率を OS 別に分離）
- 関連: PR #688（Closes #654）。before/after を別 worktree の baseline バイナリと合成ホイールで
  実測（シェル: scroll_position 0→0 が 0→24 / 偽 claude TUI: 受信報告 0 → 8）。
  副産物 #686（copy mode 居残りで打鍵が食われる）/ #687（CLI scroll の縮退とマトリクスの嘘）起票

## 2026-07-31（#653: master のアカウントをプロファイルで固定できるようにする）
- worker（#504）と対称に、プロファイルの `master_account` で master / solo の
  `CLAUDE_CONFIG_DIR` を宣言的に固定。未指定時の起動コマンドは #653 以前と完全に同一
- 起動時に「実際に使う config dir + ログインメール」を表示。tako が注入しない場合でも
  ペインは起動シェルの `CLAUDE_CONFIG_DIR` を継承するため、表示は Account / ProfileEnv /
  Inherited / Default の 4 種を区別する（一律「既定ログイン」だと #653 のドリフトを隠す。
  隔離実測で発覚して設計変更）。未登録キーはペインを作る前に登録済みキーを添えて失敗
- 関連: PR（Closes #653）。claude をスタブへ差し替えた隔離 e2e で、起動プロセスが受け取った
  `CLAUDE_CONFIG_DIR` を継承あり / なしの 2 条件 × 5 ケースで実測

## 2026-07-31（#658: セルフテストのレジストリ汚染 + 死んだ worker エントリの GC）
- 隔離漏れ: `TAKO_SELF_TEST=1` は data_dir を本番のまま使う作りで、あとから増えた
  workers.yaml（#390）と orchestrator/（ledger・projects）に逃げ先が無く、項目 72 の
  spawn 4 件が本番へ積まれていた。隔離対象を `self_test_isolation_defaults` へ集約 +
  `TAKO_ORCHESTRATOR_DIR` 新設 + セルフテスト項目 0（最初に走る隔離検査）
- GC: GUI 経路の close（× / cmd+W / タブ close）が無記録だったのを dispatch と同じ扱いに。
  加えて `workers` の列挙で「ペインも器も見えない」active に `dead_since` を刻み、
  5 分続いたものだけ closed（gone）へ倒す。1 回の観測では倒さないので過渡状態で生き物を
  落とさない。表示と GC の判定は `liveness()` に一本化
- 関連: PR #701（Closes #658）。擬似本番 data_dir での A/B 実測（修正前 = 70→71 件へ混入 /
  修正後 = ハッシュ不変）+ 人工 dead エントリの回収 + 稼働中 worker の非回収を隔離実測。
  本番の残骸 3 件（44/46/55）は死亡を 2 系統で確認して closed へ（active 9 → 6）

## 2026-07-31（#652: 復元時の claude resume が config ディレクトリを外していた問題を修正）
- 根因は 2 段のうち②だけが Windows 側に残っていた。①transcript の多 config dir 走査は #662 で
  既に入っていたが、②復元が組み立てるコマンドが素の `claude --resume <id>` で、会話の所在
  （`CLAUDE_CONFIG_DIR`）を指定していなかった。claude は config dir + cwd の両方で会話をスコープ
  するため、別 config dir の会話は `No conversation found with session ID` になる（実機実測）
- 所在を `TranscriptLocation` として返し、非既定なら代入・既定なら未設定化を前置。**前置は
  `orchestrator::agent` のシェル方言部品を通す**（mac 版 #661 の `export`/`unset` 直書きを
  そのまま移植すると PowerShell ペインで構文エラーになり resume ごと落ちる）。`$env:X = ''` は
  子プロセスから「空文字が設定済み」に見えるため `$null` 代入で消す（実測で確認）
- 投入経路も `session.write()` の書きっぱなしから `shell_send`（#640）の送達確認フローへ移した
- 関連: PR #696（Closes #652）。隔離実測で before = `No conversation found` /
  after = `Claude resume 1` + 会話復元。既定 config dir の回帰 + 実在しない/不正/無しの
  3 ケースは `新規シェル 1` でクラッシュなし

## 2026-07-31（#521: Windows の PDF プレビュー（MVP）— 抽象境界 B12 の新設）
- OS 標準の `Windows.Data.Pdf`（WinRT）を採用。PDFium は `pdfium.dll` 約 6〜11MB の配布物追加が
  MVP に見合わず、MuPDF は AGPL、pdf-rs は描画品質不足で見送り。`windows` crate は既に
  gpui / wry 経由で依存グラフにいるため Cargo.lock の差分は 1 行。macOS 実装は無改変で
  `platform/pdf/macos.rs` へ移設（トークン列一致を機械確認）し、呼び出し側から cfg が消えた
- 罠: 1 ページでも描画すると終了処理で GPU ドライバ DLL の中で `0xC0000005`。1 度描画した
  `PdfDocument` を 1 つ解放せず残すと消えることを実測し、「番人」を境界の内側に置いた
  （`TAKO_PDF_NO_DEVICE_PIN=1` で回避の要否を再確認できる）
- テキストレイヤ・しおり・リンク注釈は API が無いので `PdfCapabilities` で構造化して空を返す（→ #693）
- 次: macOS でのビルドと目視（この機ではコンパイル不可）/ 実機ユーザー目視
## 2026-07-31（#659: psmux ペインのコードページが CP932 のまま残る問題を根治）
- 器（psmux）の中のシェルは tako の ConPTY の子ではないので #655 のコードページ固定が
  届かず、pwsh 7 が無い環境では日本語が化けていた。`SessionBackend::pane_pids`
  （`#{pane_pid}`）を新設し、器を配る 1 箇所（`wrap_spawn_for_pane`）から器の内側にも
  B19 の固定を当てる。unix は `pin_needed()` で門番して問い合わせすらしない
- 検証: 実 psmux の統合テスト 2 本。`pane_pids` を空返しにすると両方落ちることを実測

## 2026-07-31（#686: psmux の copy mode 滞在中に打鍵が食われる問題を根治）
- 遡り中の打鍵が copy mode に飲まれる問題を「転送したホイールの上下差（無料・即時）で
  門番 → `#{pane_in_mode}` で非同期に確認 → 解除キー `q` を打鍵と**同じ PTY へ前置**」で解消。
  ソケット経由の cancel は打鍵との順序が保証できず、打鍵経路に器のサブプロセスを同期で
  挟むのは #212 / #168 の再来なので採らない
- 実測の要点: psmux はホイール報告を受けてから copy mode に入るまで数百 ms〜1.3 秒かかる
  （1 回聞いて終わりにできない → 間隔を空けて数回聞く）。`bind -T copy-mode Any` は
  psmux がキーテーブル未実装で無反応。排除案と全実測は #686 のコメントに記録
- 検証: 実 psmux + 実 PTY の統合テスト 2 本（**前置を外すと落ちる**ことを実測）+
  隔離 GUI E2E 3 種（通常シェルで打鍵到達 / マウス要求 TUI へ `q` が漏れない / #659 の日本語）

## 2026-07-31（#519 / #687: psmux へ読み取り専用の到達手段を新設し、Windows の採取系を通した）
- 役割 B を**読みと書きに割った**（`DetachedCapture` を新設し `DetachedAccess` をその
  supertrait に）。psmux は `capture-pane` は動くが `paste-buffer` / `send-keys -H` が
  信頼できないので、フル実装して送出だけ Unsupported を返すのではなく**型で分ける**。
  `capabilities()` に `detached_capture` が増え「器あり・採取あり・送出なし」を表現できる
- 読むだけの 4 経路（read フォールバック / report の scrollback / worker_status の
  recent_output と履歴サイズ）を `reach::detached_capture` へ。before は全部
  「到達手段が無い」で即エラーだった
- #687: psmux ペインの `tako scroll` は外側 alacritty が alt screen（履歴 0）で
  動かせる余地が無く、応答だけ成功を装っていた。**ユーザーのホイールと同じ in-process の
  PTY へ SGR 報告を書き、位置は読み取り専用の到達で読み戻す**形に。粒度（psmux は実測 3 行）は
  1 往復目で学習してキャッシュ、最下部へは撃ち足して copy mode 居残り（#686）を避ける
- psmux 実測で確定した非互換: `-J` は無視される / **複合コマンド（`;`）は後半が捨てられる** /
  `#{history_bytes}` は空。`#{scroll_position}` `#{pane_in_mode}` `#{alternate_on}` は動く
- 関連: PR（Closes #687、Refs #519）。before/after を別 worktree の baseline バイナリで実測
  （report: エラー → 208 行 + 日本語折返し / scroll: `offset:0` 固定 → 器の実位置に一致）。
  マトリクスは `tako_orchestrator_report` Pending→Degraded、`tako_scroll_pane` Supported→Degraded

## 2026-07-31（#697: Windows の蓋閉じ継続 — #524 の「API が無い」を実測で否定）

- #646 が `lid_control_supported()` を macOS 固有として落とした前提（「Windows に相当する
  API が無い」）は誤り。`GUID_LIDCLOSE_ACTION` は `powercfg /q` に出ないだけ（定義側
  `Attributes = 1` で hidden）で、**非管理者のまま**読み書き・適用・復元できると実測。
  macOS の sudoers 登録に相当する初回セットアップすら要らない
- `platform::lid`（B9 の蓋ぶん）を新設し電源プランの lid action を倒す / 戻す。Modern Standby 機は
  「アイドル」と「蓋」が別の引き金なので、#646 の電源要求と本実装の**両方**で初めて蓋閉じ継続が成立する。
  判定は macOS と同一の純粋関数（`should_disable_lid_sleep`）を通し、倒すのは AC レールのみ
- 残留対策 3 段: 倒す前に元値をディスクへ保存 / 正常終了で即復元（**電源要求と違い OS が回収しない**）/
  起動時の残留復元。ユーザーが手で変えていたら復元しない。文言も「sudoers」から
  「初回セットアップの要否」へ抽象化し、Windows に macOS 専用の案内が出ないようにした
- 関連: PR #706（Closes #697）。実機テスト 5 本（独立 FFI プローブで電源プランを外から観測）+
  単体 21 本。終了時復元は**修正を外すと FAILED になること**まで確認。マトリクスの note を
  実態へ直し doc 再生成。tako-control 836 passed / 失敗 11 件は baseline と同一
- 次: macOS のコンパイルは未検証（`ring` が絡み Windows からクロスチェック不可）→ CI 待ち。
  実際に蓋を閉じての確認は manual-checks.md に手順を追加

## 2026-08-01（#709: claude ログインアカウントの高速切替）

- 部品（accounts.yaml #504 / ログイン状態読み取り #653 / master_account・worker_account）は
  揃っていたが**束ねる導線が無かった**（ログイン状態は読めるのに一覧に出ず、割り当ては
  `profiles set --master-account` と長く、config dir 作成と `/login` は完全に手作業）。
  `tako account`（list / add / remove / show / login / use）へ 1 本化。MCP は**新ツールを
  増やさず** `tako_orchestrator_accounts` に action を足して 1:1 を保つ
- 着手前に **main のアカウントモデル（#512 / #543）だけ先に移植**した。この機のブランチは
  `AccountEntry.config_dir: String` のままで「既定の資格情報を使う」を表現できず、
  ログイン導線がまさにそこを踏み抜くため（claude は `CLAUDE_CONFIG_DIR` が**設定されている
  だけで**資格情報エントリを分ける = 既定パスの明示は別アカウント扱いになる）。
  `Option<String>` + `inherit` + `AccountConfigDir` + `EnvPlan`（代入と **unset** を区別）へ
- ログイン状態は 3 値（`missing` / `logged_out` / `logged_in`）。`missing` を分けるのは
  次の一手が「作るところから」か「認証だけ」かで変わるため。壊れたエントリは握り潰さず
  `invalid` + 直し方を出す
- 検証: 隔離 `TAKO_ORCHESTRATOR_DIR` + ダミー config dir で 4 状態を実測、割り当ての
  profiles 書き込みと未登録キー・排他違反・破損 YAML のエラー、`inherit` が起動コマンドで
  **unset** になること（代入に化けたら FAILED になるテスト）まで確認
- 関連: PR（Closes #709）。tako-control +14 / tako-app +3 テスト。fmt / clippy は baseline と同一
## 2026-07-26（#511 / #512: アカウント切替の残欠陥 2 件）
- #511: CLI `orchestrator spawn / run --account` を追加（dispatch には #504 で入っていたが
  CLI が account: None 固定だった）。MCP `tako_orchestrator_run` の schema にも account を追加
- #512: accounts.yaml に `inherit: true`（CLAUDE_CONFIG_DIR を設定しない）を新設。
  `AccountConfigDir { Path | Inherit }` + `EnvPlan { exports, unsets }` で「未設定」を型で表し、
  spawn は `unset CLAUDE_CONFIG_DIR;` を前置する（direnv 対策）。既定パス明示の登録は警告
- 検証: 隔離 GUI + 実 claude で受け入れ 5 件を実測（inherit worker がログイン画面を出さない /
  direnv 注入に unset が勝つ / MCP・CLI 双方の spawn・run で account 適用）。品質ゲート全緑
- **事故**: 検証テストの変数シャドウイングで `~/.claude-univ` を削除（復旧不能・要再ログイン）。
  一時ディレクトリ配下を assert してから消す `remove_temp_dir` を入れて再発を構造で防止

## 2026-07-26（#547: master_account が master 起動に適用されない問題）
- `build_master_cmd`（tako master / solo）と handoff の新 master を
  `resolved_env_plan_for_master()` 経由へ。#512 の inherit（unset）もそのまま効く。
  未登録アカウント名は起動前に Err。CLI 起動時に「アカウント: <名前>（config dir: …）」を表示
- 検証: 隔離実測で univ = プロセス env に `CLAUDE_CONFIG_DIR=~/.claude-univ`、
  personal(inherit) / solo / master_account 無し = 未設定を `ps eww` で確認。
  ユニット 5 本は修正を戻すと 4 本落ちる（検出力実証）。品質ゲート + クロス check ベースライン不変

## 2026-07-26（#548: accounts の CLI 追加でアカウント系の 1:1 欠落を解消）
- `tako orchestrator accounts list/show/add/remove` を追加。実体は
  `dispatch_orchestrator_accounts` を pub 化して共用（layout と同じローカル呼び出し）
- 検証: 同一 data_dir に対する CLI と MCP の出力を diff して list / show / add（警告つき）/
  remove が完全一致。`--inherit` / 既定パス警告 / 排他エラー / 壊れたエントリ表示も実測。
  CLI 登録アカウントが spawn で解決されるところまで通し確認。品質ゲート全緑

## 2026-07-26（#530: spawn 初期プロンプトの消失を根治）
- 根因は疑いにあった「シェル段階の誤判定」ではなく、**claude の番号付き選択ダイアログ**
  （新 config dir の初回テーマ選択 `❯ 2. Dark mode ✔` / ログイン方法選択）の選択カーソルを
  `input_line` が入力欄と誤認していたこと。`CLAUDE_CONFIG_DIR` 切替時に必ず出るため
  account env 注入つき spawn 特有に見えていた。before 実測でテーマ選択が自動確定されて
  次画面へ進み（t=12.99 → 13.33）プロンプトが消える様子を撮影
- 修正: 文言非依存の `is_choice_dialog`（最下部プロンプト行が `N. …` + 選択肢 2 件以上）を新設し
  `input_line` から除外 / 送達の証拠を「入力欄が空」から「貼り付けが入力欄へ反映された」へ /
  未達は `prompt_delivery=undelivered` + `prompt_delivery_failure` + `resend_command` で報告
- 検証: 10 連続 spawn で消失ゼロ・registry 全件 delivered、fresh config dir では
  ダイアログを確定させず undelivered(choice_dialog) + 再送コマンド提示。品質ゲート全緑（1360）+
  隔離セルフテスト FAILED 0。`claude_tui_e2e` の 2 件失敗は main 時点で同一 = 回帰ではない

## 2026-07-27（#553: パネルビューの語彙を GUI 表示名へ統一）
- GUI は fleet / orch / git、CLI / MCP は tmux / orch / git で食い違い、画面に見えている語で
  操作できなかった。`PanelViewWire::Fleet` を正式値化し、語彙の正本（`VALUES` /
  `LEGACY_VALUES` / `parse` / `values_hint`）を protocol.rs に集約して CLI・MCP が同じ表から引く形へ。
  応答 JSON は旧称入力でも `fleet` に正規化。tako-app の `PanelView::Tmux` も `Fleet` へ改称
- 後方互換: `--view tmux` と MCP `view:"tmux"` は受理継続（`serde(alias)` で IPC の JSON も同様）。
  不正値は CLI が possible values + 「a similar value exists: 'fleet'」、MCP が
  「fleet | orch | git。tmux は fleet の旧称」を返す（#553 案 2）
- 検証: 品質ゲート全緑（1377）+ 隔離セルフテスト FAILED 0 + 隔離インスタンスへの実測
  （fleet / tmux / git 切替・MCP 3 経路・tools/list の enum）。GUI スクショは蓋閉じで取得不可、
  代わりにセルフテスト 49 が `app.panel_view == PanelView::Fleet` と応答 `view=="fleet"` を同時判定

## 2026-07-27（#550 + #559: ファイルツリーの初回印象と新規作成インライン入力）
- #550: ドット始まりを既定で非表示（表示時フィルタ = トグル即反映。ルート見出しは対象外）+
  トグル 3 経路（見出しの目アイコン / 右クリック / 設定画面の外観）+ 増えたルートを自動展開して
  先頭へ（既存の並びは維持しポーリングで暴れない）。settings.json `show_hidden_files` 永続化 +
  CLI `tako panel --show-hidden` / MCP `tako_panel` の `show_hidden` と 1:1
- #559: インライン入力欄を「展開済み子孫を飛ばした末尾」→「確定後にその項目が並ぶ位置」へ。
  位置計算は純関数 `sidebar::inline_insert_position` に切り出し、インデント規則を通常行と一致。
  作成先の行を強調 + プレースホルダ + 確定後は `FileTree::refresh_dir` で即反映
- 実測で判明した訂正: **VSCode / Zed の既定は `.git` 等の個別除外であり、ドット全体は隠さない**
  （実機確認）。Issue 本文の「VSCode と同じ既定」は不正確なのでコメント / requirements を訂正。
  VSCode の New File 入力位置も「親の真下」ではなく「ファイル群の先頭」だったので実装を合わせた
- 関連コミット: `fe5e341`（PR #565 squash merge）。品質ゲート全緑（1384）+ セルフテスト項目
  83 / 84 新設（修正を戻すと FAILED になることを実測）+ CLI・MCP・永続化・GUI 実クリックを隔離実測。
  証拠は `~/dev/tako-evidence/550/`
- 既知: 合成キー入力は IME に吸われるため入力欄への打鍵は GUI 未検証（セルフテスト 84 で代替）。
  `cargo fmt --all --check` が `keybindings.rs:248` で落ちるのは **main 由来**（#546 merge 時点から）

## 2026-07-27（#558: 事前信頼の書き先を claude の config dir 配下へ）
- 根因: claude は `<config dir>/.claude.json`（既定 `~/.claude/.claude.json`）を読むのに、
  tako はホーム直下の `~/.claude.json` へ書いていた。承諾直後の diff で「config dir 側だけが
  変化しホーム直下は無変化」を実測。事前信頼(#32)と bypass 事前承認(#407)が両方無効だった
- 修正: `config_json_paths`（config dir 配下 → 既定。旧ファイルは存在時のみ併記）+
  `ensure_trusted_in` / `ensure_bypass_accepted_in` + `EnvPlan::claude_config_dir` で
  spawn / handoff / git resolve から起動先の config dir を渡す。e2e の後始末も同じ規則へ
- 検証: `claude_tui_e2e --ignored` が 2 failed → 4 passed（109 秒 → 27 秒）。
  アカウント指定 spawn の書き先を before/after 実測。品質ゲート全緑（1389）

## 2026-07-27（#551 / #560 / #561 / #562: git タブの UX 4 件を一括改善）
- #551 本文順を 変更 → コミット → ブランチ → リモート → diff へ + 既定折りたたみ見直し（案 1/2/3）、
  #560 変更ファイル行クリックでプレビュー（`open_file_row` = dispatch OpenFile 経由）、
  #562 マージボタンの常時表示 + 案内行 + ブランチチップからの導線
- #561 根因は実測で確定 = **変換がターミナルペインに束縛され、下線・候補ウィンドウ・unmark の
  確定先がすべてターミナル側へ向いていた**（`bound_pane=PaneId(1)` / `candidate_bounds=(11px,87px)` /
  `commit_msg_after_unmark=""`）。`AppTextInput` + `ImeComposition.app_input` で宛先を型にして根治
- 副産物: UI アイコン定数の `EMBEDDED_ASSETS` 登録漏れ検査テストを新設し、既存 remote.svg が
  無言で描かれていなかったのを検出・修正。セルフテスト 85 / 86 / 86b を新設
- 関連コミット: `b62c325`（PR #570 squash merge）。品質ゲート全緑（1393 tests）+ パリティ 0 エラー +
  隔離セルフテスト `TAKO_APP_SELF_TEST_OK` + 隔離 GUI 実クリック（証拠 ~/dev/tako-evidence/560/）
- 次: tako 再起動 → #561 の実 IME 目視（この機は日本語入力ソース未有効）と #562 の導線目視。#562 は open 維持

## 2026-07-27（#574 + #567: CI 復旧 + stale TAKO_PANE_ID の master 起動 fallback）
- #574: 45 日ぶりの CI 実走で腐敗が発覚。ci.yml の mac / Win 両ジョブに PWA ビルド工程を追加
  （rust_embed が埋め込む `web/tako-remote/dist/` が CI では未生成だった）。Windows は**テスト
  ステップのみ** continue-on-error（#583 完了までの暫定）。**以後の合格条件 = macOS 全ジョブ緑**
- #567: stale な `TAKO_PANE_ID` を持つシェルからでも `tako master` / `solo` が起動できる fallback
- 関連コミット: `cb2d06e`（PR #580）/ `1a5b91d`（PR #573）。CI は macOS / Windows / Pages 全 pass。
  副産物起票: #583（Windows で tako-control テスト 19 件が POSIX 前提で fail、#467 の子）

## 2026-07-27（#566: ペイン close の確認ガード + 発生源の監査記録）
- cmd+W を × と同じ確認経由にし、確認対象は「失うものがあるペイン」（role 付き / Running /
  子プロセスあり）に限定。`CloseOrigin` 型で close の発生源（kbd / ui / dispatch + caller_role）を
  pane_log マーカーへ記録。副産物: config.yaml が無い環境では **#172 以来 close 確認が既定 OFF**
  だった `SetupConfig` の serde default 無視バグを発見・修正
- 関連コミット: `e59ea16`（PR #581 squash merge）。品質ゲート全緑（1400）+ セルフテスト 73a2/73f/87 +
  実クリック証拠 ~/dev/tako-evidence/566/

## 2026-07-27（#572: busy 中に人間が打った指示の消失を根治）
- 根因を隔離実 claude で確定: **claude は生成中の打鍵を入力欄ではなく内部キューへ入れる**
  （ターン終了時に送信）。その間の入力欄は空で dim のヒント `Press up to edit queued messages`
  が出る。tako はこれを「残留テキスト」と誤認し、Enter 単独送達が no-op の Enter を 5 回
  空撃ちして `verified=false`、`read` も `style=ghost` + テキストありに見えていた
- 是正: ①「入力欄が空か」を **dim 属性**で判定（tmux 経路は `capture-pane -e`。文言リストでは
  AI のゴースト提案を網羅できない）②キュー滞留を `queued_messages_pending` で検知し
  `read` / `worker_status` / watch イベントへ公開 ③生成が止まっているのにキューが残っていたら
  `Up` → `Enter` で送り出す。生成中かは `is_busy` の文言ではなく **画面が変化していないこと**
  （実測で 120 行のリスト生成中に `is_busy` が false を返し救出が暴発した）
- 検証: fmt / clippy(-D warnings) / test 全緑（1398）+ 実 claude e2e 新設（修正を戻すと
  FAILED になることを実測）+ 隔離セルフテスト。既存 e2e 4/5 通過（残り 1 は `/tmp` が
  信頼済みという環境要因で main でも同じく失敗）

## 2026-07-27（#571: orchestrator watch が WORKER_IDLE を発火しない問題の根治）
- 3 層の欠陥を実測で確定して全部潰した: ①`claude agents --json` をプロセス環境の
  `CLAUDE_CONFIG_DIR` ごと実行しており、アカウント env つきペインから起動した GUI では
  他アカウントの worker が「存在しない」ことになる（本番汚染下の実測で 1 件 → 8 件）
  ②画面フォールバックが `screen_looks_busy || has_children` で busy に上書きしていたが、
  エージェント TUI 自身がペインシェルの子なので has_children は常に true = 構造的に idle を
  出せない ③claude のフッターが 8 行あり `screen_looks_busy` の末尾 5 行がスピナーに届かない
  （②を外すと今度は偽 IDLE になる関係）。併せて claude の実 status 語彙（idle / busy）への
  正規化と、agents が状態を返せないときの `status_source` 降格も修正
- 関連: PR #578 squash merge（Closes #571）。fmt / clippy / 1408 tests 緑 +
  実 claude e2e（`issue571_e2e`）が修正前 Timeout(60s) → 修正後 Idle(13.9s) +
  隔離 GUI + 実 CLI watch で WORKER_IDLE を idle から 16 秒で発火（復元またぎ・screen 経路も確認）
- 副産物: permission ダイアログ待ちが WORKER_QUESTION になる（`waiting` へ到達する経路が
  claude では存在しない）を #577 に起票。Stop hook error は無害と確認（Issue の疑いは外れ）

## 2026-07-27（#549: 初回起動のウェルカムバナー + ⌘K パレット導線）
- 初回起動のみのバナー（setup / master のその場実行）+ パレットに「セットアップを実行 / 設定を
  開く / master を起動」。`welcome_dismissed` を settings.json 永続化、破損 settings でも安全。
  MCP `tako_welcome` + CLI 1:1
- 関連コミット: `6dfd34b`（PR #597 squash merge）。バナー見た目の GUI 目視は画面ロックで未取得
  （再起動後にユーザー目視）。PATH 問題の残りは #601 へ分離

## 2026-07-27（#589: ファイルツリーのインデントガイド線の途切れを根治）
- 根因 = 行の border-left では自分の深さの線しか描かれず、子孫行の区間で祖先の線が丸ごと欠けていた。
  祖先ぶんの縦線もまとめて描く方式へ。visual-test に dark / light / スクロールのピクセル連続性検査を常設
- 関連コミット: `c601417`（PR #593 squash merge）

## 2026-07-27（#552: AI 自動リネームの品質改善 4 点）
- 同一タブ 5 分下限 / 一時的失敗（command not found 等）を材料から除外 / 出力言語を UI 言語に固定
  （簡体字 115 対置換 + CP932 字種検査）/ 自動命名直後のピン印ワンクリック固定（`tako tab pin` + MCP）
- 関連コミット: `8667da7`（PR #598 squash merge）。副産物 #599 起票

## 2026-07-27（#590: リモートインジケータの常時表示 + GUI からの起動）
- daemon 非稼働時もステータスバーに表示、クリックで起動導線 + 未セットアップ案内、稼働中は従来の
  端末一覧。MCP ツール件数ずれ（126→127、main 由来）と SIGTERM 経路の根治も同梱
- 関連コミット: `818b07c`（PR #596 squash merge）

## 2026-07-27（#599: セルフテスト項目 87 が worker ペインで落ちる問題）
- 判定を部分一致から `CloseOrigin::marker()` 系 API で生成した期待値との完全一致へ。
  `close_marker_reason()` を pane_log に新設し、書き出しと判定が同じ定数を共有
- 関連コミット: `5eec43e`（PR #605 squash merge）。テストのみの変更で install 不要

## 2026-07-27（#594 + #595: リリース配布物のプラットフォーム対応）
- アセット命名規則の正を `tako-core::platform::release_assets` に新設（シェル写しは実行結果一致の
  同期テストで拘束）。#595 = 更新候補を「自 OS アセットを含む最新リリース」へ（旧実装は assets を
  見ず URL を合成していた）。実リリース 28 件の総当たりで macOS 判定の完全一致を固定。
  #594 = release.sh のノート生成（ダウンロード表 / Known limitations）+ `--notes-only` / `--update-notes`
- 関連コミット: `a425a63`（PR #606 squash merge）。副産物: `--promote` が set -e で落ちるバグ修正

## 2026-07-27（#600: 入力予測 — zsh-autosuggestions の同梱注入・既定 ON）
- v0.7.1（MIT）をバージョン固定同梱し、シェル統合（ZDOTDIR）経由で tako 内の zsh 限定・最初の
  precmd で読込。ON/OFF は状態ファイル方式で稼働中ペインへも次プロンプトから反映。3 経路 1:1
  （設定画面 / `tako autosuggest` / MCP tako_autosuggest）+ 二重注入ガード + THIRD-PARTY-NOTICES
- 関連コミット: `e737117`（PR #607 squash merge）。副産物 #608 起票（表示言語グローバル競合フレーク）

## 2026-07-27（#577: permission ダイアログの WORKER_PERMISSION 検知）
- Issue の機序を実測で訂正: agents 解決成功時は waiting が返る。真の欠陥は**画面推定経路**
  （agents に載らない環境）。ダイアログが画面に実在すれば waiting へ格上げ +
  `detect_permission_dialog` に実在検査（入力欄を奪う構造）を必要条件化。question とは排他
- 関連コミット: `27ae97c`（PR #609）+ `38ab099`（PR #612 = e2e の信頼エントリ残骸の後始末）

## 2026-07-27（#601: tako 内シェルへの CLI PATH 自動注入）
- FR-2.4.6 新設。判定はシェル側「rc の後」の一点（zsh precmd / bash PROMPT_COMMAND / fish
  fish_prompt）で、tako が他に見つからないときだけ PATH 末尾へ追加。rc 非侵襲・
  `TAKO_NO_PATH_INJECTION=1` で無効化。`tako_check_health` に `injected_cli_dir`
- 関連コミット: `c2c9350`（PR #613 squash merge）。案 2（外部ターミナル向け設置）は FR-2.14.5 に残

## 2026-07-27（v0.6.0 安定版リリース）
- CHANGELOG に `[0.6.0]` を新設（v0.5.9 以降 = nightly 0.5.10〜0.5.13 + 未リリース 2 ブロック +
  本日の 12 件を日英併記で統合・タグ規約準拠）。`[Unreleased]` を空に、未公開だった
  `[0.6.0-test.1]` 節に「未公開」注記。Cargo.toml / lock を 0.6.0 へ bump
- tag `v0.6.0` + GitHub Release を **Latest** で公開（#594 の新ノート機構を初適用 =
  実アセットからダウンロード表 + macOS 手順を生成。Known limitations は Windows アセットが
  無いため設計どおり非表示）。cask 0.5.9→0.6.0（`brew fetch` で sha256 実検証）、
  /Applications = 0.6.0、0.6.0 隔離インスタンスで `update check` = `{"available": false}` を実測
- 関連コミット: `29837da`（tako）/ `acf412e`（homebrew-tako）、tag `v0.6.0`
- 次: GUI 再起動で本番反映 → 目視確認 → #434 の宣伝タスク

## 2026-07-28（夜間バッチ: #614 / #615 / #616 / #620 / #621 / #608 / #619 / #513 / #625）
- #614: 予測の確定案内 `[→ か Tab で確定]`（既定 10 回で消灯）+ ゴースト表示中のみの Tab 確定
  （POSTDISPLAY の 2 関門ラップ方式）。PR #622 = `6f5e75f`
- #615: リモートカードをインジケータ直上へアンカー（paint フックで矩形記録 + クランプ）+
  起動/停止トグル（台数付き確認）。PR #618 = `b19ab8c`。副産物 #619 起票
- #616: アップデート UI 刷新 — 下部バーから撤去 → 専用画面（設定ウィンドウ流用）+ 上部通知カード
  （× で同一バージョン再表示抑止）。PR #630 = `41ff25e`。**見た目のスクショは蓋閉じで未取得**
- #620: docs 全面刷新 — CLI 68 / MCP 128 の全数機械照合で乖離ゼロ化、リリースページ再構成、
  モダン化（絵文字→SVG 等）、モバイル実バグ修正。PR #626 = `45b7bba`、ライブ反映確認済み
- #621: リモート選択画面の大改善 — 根因 = プレビューが最古の履歴の先頭を描画。daemon に
  `preview`/`activity`/`error` を同梱しタブグループ + 状態ピル + チップへ。チャット画面不変。
  PR #629 = `9a88a4c`。**検証テストが本番 remote state を消すバグも発見・同 PR で根治**。
  副産物 #632/#633 起票
- #608: 言語グローバル競合フレーク根治 — gate を純関数化 + 残り 3 本を panic 安全な lang_guard で
  直列化。before 26/60 → after 0/300。PR #624 = `9f625c7`。副産物 #625 起票
- #619: daemon 停止後の defunct — 根は起動側の `mem::forget`（+ ゾンビにも kill(pid,0) が成功し
  停止が誤失敗と報告されていた）。reap スレッド + `has_terminated` へ。PR #631 = `e0d4ce1`
- #513（mac 側）: `tako config` / MCP `tako_config_share` — ホワイトリスト fail-closed +
  番犬テスト、秘匿はフィールド単位分離、可搬性は `~` トークン化。setup は `--review` でのみ
  オプトイン。PR #636 = `3da4c41`（Refs #513、Windows 実機配線は #467 と合流で open 維持）
- #625: scroll テストフレークから独立 3 機序を根治（偽の待ち条件 / 生存サーバー誤 kill /
  ensure_conf 非アトミック = 本番経路の実バグ）。before 2/30 → after 0/80。PR #637 = `8fb2a7c`。
  副産物 #638 起票
- 運用: API エラー 2 件は resume で復旧 / 選択ダイアログ 1 件は master 裁定（PR に含める）/
  GUI は 01:38 と 03:0x に再起動（env -u 必須の知見は auto-restart-permission メモリ参照）

## 2026-07-30（#656: Markdown プレビューの表示品質を全面改善）
- GFM テーブルを表形式で描画（罫線 / ヘッダ帯 / ゼブラ / 列アライメント / 表示幅比の列配分）+
  見出し 6 段・コード（ライトは輝度クランプ）・インラインコード・引用ネスト・リストマーカー・
  図形チェックボックスの配色刷新。選択・コピーはセル単位（x でヒットテスト解決）
- レイアウトの根治 2 件: 全 md ブロックへ `flex_shrink_0`（overflow_hidden は flex 自動最小
  サイズを無効化 → 表が潰れて重なる）/ 表セルの StyledText を `min_w(0)` で包む（GPUI の
  min-content 幅は折り返しなし 1 行分のため縮まず隣セルへ溢れる）
- 関連コミット: PR #662（`1f64bc7` + `dcdaad8`）。dark / light / 狭幅 × 全文スクロールの
  visual-test + 単体 11 本 + 隔離セルフテスト `TAKO_APP_SELF_TEST_OK`

## 2026-07-30（#668: visual-test のインデントガイド節が main で失敗していた問題）
- 根因は検査側: 走査範囲が固定の論理 px（115..420）で、初回起動バナー（#549）が上に
  載った分サイドバーのヘッダがその範囲へずり込み、**パスボックスの枠（border_subtle =
  ガイドと同色）をガイドの一部と誤認**して連続している線を「途切れ」と判定していた
- 修正: 走査範囲を**実際に描かれた行矩形**（新設した `filetree_scroll_handle` の
  `bounds_for_item` + スクロールオフセット補正）から深さごとに導出。バナーは検査前に閉じる。
  `dark-scrolled` の合成ホイールがリストへ届いていなかったのもハンドル経由へ直した
- これで visual-test が**全節完走**（#152 PDF / 構文色 / #159 サブラインスクロールは
  この失敗以降そもそも実行されていなかった）。実装を #589 前へ戻すと FAILED になることも実測
- 関連コミット: PR #670

## 2026-07-30（#669: コードプレビュー（非 md）の構文色がライトテーマで読めない問題）
- syntect のテーマは `base16-eighties.dark` 固定で、非 md のコードプレビューはライトでも
  ダーク配色のまま（既定文字色 1.36:1）。#656 の輝度クランプは md のコードブロック限定だった
- 修正: 変換を `Theme::adapt_syntax_color`（+ `SYNTAX_LIGHT_MAX_LUMINANCE` = 0.12）へ切り出し、
  非 md と md の両経路を同一関数に通した（見た目の一貫性を構造で担保）。描画時変換なので
  再ハイライト無しでテーマ切替に即応し、ダークは原色のまま
- 検証: 実ハイライタ出力の全色走査テスト（Rust / Python / C++、`background` と `mantle` 両面で
  4.5:1）+ visual-test 新節（light_readable 3360/3290・theme_changed 71568/107352・
  dark_roundtrip_diff 0・span_colors_stable）。実装を戻すと単体 2 本と visual-test が FAILED
- 既知の限界: 硬いクランプなので**ライトではコメント灰と既定文字色がほぼ同じ濃さ**になる
  （`#63625a` vs `#63625e`）。色相で分かれない灰系は見分けが付かない → 別途 Issue 化を提案
- 関連コミット: PR（Closes #669）

## 2026-07-30（#680: md プレビューのリンク ⌘+クリック + コードブロックのコピーボタン）
- `MdSpan.link: bool` → `link_url: Option<String>` で遷移先をモデル保持し、⌘+ホバー装飾
  （下線実線化 + accent 背景）+ ⌘+クリックで `os_integration::open_url`。**開くのは
  http / https のみ**（`tako_core::md_links::browser_url` が正）。当たり判定は
  `TextLayout::index_for_position` の `Ok` だけ = ⌘ 無しの選択は不変。索引の正は
  `md_document_links` 1 本で render とCLI 一覧が同じ並びを共有
- コードブロック右上にコピーボタン（装飾なし全文 + 2.2 秒「コピーしました」）。
  **`opacity(0)` + `group_hover` の「ホバーで初めて現れる」方式は実機で復帰せず
  ボタンが一度も見えないと実測**したので常時表示（待機中はアイコンのみ淡色）へ
- CLI / MCP: `preview-link-list` / `preview-follow-link` を md へ拡張（応答に `kind`）+
  `tako preview-copy-code` / `tako_preview_copy_code` 新設（131 ツール）
- 関連コミット: `9758a6b`（PR #685 squash merge）。品質ゲート全緑 + セルフテスト項目 90 +
  visual-test 全節完走（3 連続）+ 実マウスの ⌘+クリックでローカル HTTP サーバへの
  実アクセスをログで確認。副産物: `wait_for_preview_maps` の PDF 待ちを 2s → 4s

## 2026-07-31（#691: GUI ライク表示モード（初心者向け UI）の詳細仕様策定 — docs のみ）
- 仕様書 `.agent/plans/2026-07-gui-mode.md` 新設（243 行）: グローバル `ui_mode` トグル
  （テーマボタン隣 + settings.json + dispatch/CLI/MCP 1:1）/ スターター 3 ボタン
  （welcome バナーと同じ「シェルへ `tako master`/`solo` 書き込み」方式）/ claude ペインの
  チャットビュー（transcript 正規化 + agents ctx% + Send/Respond = PWA と同一データ源の再利用）/
  表示レイヤのみの切替で PTY・tmux・persist 不変を裏付けつきで明記。フェーズ G1〜G4
- 関連: エピック Issue #691、PR #692 squash merge（`200d889`）。CI は macOS 全ジョブ緑で合格
- 次: G1（モード基盤 + スターター）の worker 割当は master 判断

## 2026-07-31（#694: GUI モード G1 — モード基盤 + スターター 3 ボタン）
- `ui_mode`（既定 terminal）を settings.json / dispatch `UiMode` / CLI `tako ui-mode` /
  MCP `tako_ui_mode`（132 ツール）へ。判定表は `tako-core::ui_mode` の純関数
  （材料は OSC 133 の Idle + role なし + sleep_guard の子プロセスキャッシュ = 新規ポーリング無し）、
  分岐は render_pane の 1 箇所（**PTY リサイズの後**）。スターターは `starter.rs`
- 仕様との差分 1 件: 「コマンド入力へ」の AI 等価操作が無いと開発不変条件を満たせないため
  `UiMode` に `release` / `restore`（揮発・非永続）を追加。仕様書 §1.4 / §2.2 に反映
- 検証: 品質ゲート全緑（app 290 / cli 46 / control 852 / core 497 / parity 10）+
  隔離セルフテスト `TAKO_APP_SELF_TEST_OK`（項目 93 新設）+ visual-test 新節
  （terminal 帯 0 → GUI dark 21097 / light 18416 の可読ピクセル）+ 隔離実機の
  CLI / MCP / 再起動往復。既存ファイルの削除行は 2 行（タブバー幅・ツール数）のみ
- 次: G2（チャットビュー読み取り）。`PaneDisplay::Chat` と `claude_chat` は配線待ちで用意済み
## 2026-07-31（#690: アップデート詳細のリリースノートを Markdown レンダリングへ）
- md の**幾何とテーマ色を `md_view.rs` の 1 実装へ集約**し、プレビューペインと
  アップデート詳細画面が同じ `render_block` を通る形に。差は `MdTextSink`
  （選択・検索・TextLayout の控え・コピーボタン）だけ。パースと リンク索引も共有
- アップデート詳細のノートを md 描画 + ⌘+クリックでブラウザ（http/https のみ）へ。
  **GPUI の `TextLayout::bounds()` は prepaint 前に呼ぶとアプリごと panic する**ので、
  ヒットテストは canvas の paint で立てた「描き終わった世代」だけを対象にした
- 検証: 品質ゲート全緑（1701 tests）+ 隔離セルフテスト `TAKO_APP_SELF_TEST_OK`（項目 90(f)
  新設）+ visual-test 全節完走（新節 update-notes = **実リリース v0.6.2 のノート本文**を
  実ピクセル検査）。生テキストへ戻すと単体 / セルフテスト / visual-test の 3 つとも
  FAILED になることを実測（検出力の実証）

## 2026-08-01（#721: プロファイル（master / solo）の GUI 編集）
- 設定画面に「プロファイル」タブを新設（8 タブ目）。種別切替・一覧・全項目フォーム編集・
  新規/複製/削除（確認つき・default は不可）。書き込みは既存 `OrchestratorProfiles` dispatch 経由で
  UI 直書きなし（#169 の config_io と CLI/MCP 検証がそのまま効く）。FR-4.7 として要件化
- dispatch を kind（master/solo）+ create/copy/delete + projects へ拡張し CLI（`profiles
  create/copy/delete`・`--solo`・`--projects`）と MCP へ 1:1 公開（ツール数は 132 で不変）。
  参照整合性の警告は `orchestrator::profile_warnings` の 1 実装で list/show/set 共通
- 検証: 品質ゲート全緑（1752 tests）+ 隔離セルフテスト完走（項目 96 新設。refresh を外すと
  FAILED になることを実測）+ 隔離 GUI で `tako master -<新規>` の実起動・使用中プロファイルの
  編集/削除・壊れた yaml・全項目 roundtrip を実測。GUI スクショは蓋閉じで未取得

## 2026-08-02（#725: チャットビューのテキスト選択・コピー）
- 会話本文をドラッグ選択 + ⌘C / ⌘A。座標系は `ChatTextIndex`（行 → プレーンテキスト +
  実描画 `TextLayout`）を 1 ペインぶん通しで採番するので**発話をまたぐ選択**が成立。
  ヒットテスト（`preview_text_layout_hit_test`）と切り出し（`selection_text`）は
  プレビューと同一実装に 1 本化した = 見えているものとコピーされるものが一致する構造保証
- 発話の右に固定列 + コピーボタン（画面と同じプレーンテキスト・折りたたみ中でも全文）、
  チャット内 md コードブロックにも #680 と同じコピーボタン。CLI `tako chat copy` +
  MCP `tako_chat_copy`（133 ツール）へ 1:1。ドラッグ選択自体はポインタ操作なので CLI 非公開
- 検証: 品質ゲート全緑（1780 tests）+ 隔離セルフテスト項目 98（索引 / ヒットテスト /
  ⌘C / コピーボタン / MCP / 折りたたみ / 空発話 / ドラッグ中スクロール抑止 / terminal 復帰）+
  visual-test（**合成マウス `PlatformInput` のドラッグ**で選択 → dark 5413・light 4239 px の
  塗り + pbpaste 一致 + コピーボタン 727 px）+ 隔離 GUI の実 claude で 3 経路 pbpaste 一致。
  検出力は「索引経由をやめる」「マウス配線を外す」の 2 通りで FAILED を実測

## 2026-08-02（#738: 設定画面プロファイルタブの描画崩壊を根治）
- 根因は taffy が**幅 auto の親の中の `flex_wrap` コンテナ**の max-content 幅を「一番広い項目
  1 個ぶん」として返すこと。チップが 1 個ずつ縦に折り返される一方、行の高さは 1 行ぶんで
  見積もられ、伸びたチップ群が次の行へ重なっていた（実測: effort 群 5 段 149.5px / 行 39.5px）。
  チップ行を `row_wrapping()`（行の残り幅を持つ右寄せセル）+ `w_full` へ移して幅を確定
- 再発防止: visual-test に `profiles-form` 節（実測矩形の総当たり = 重なり / 枠外 / 幅溢れ +
  合成マウスのクリック一致 + 低いウィンドウのスクロール到達 + 他タブの巻き添え検査）と
  GPUI 非依存の `form_layout`（unit 6 本）。before 14 重なり → after 0（dark / light / 最小サイズ）

## 2026-08-02（#737: チャット入力欄の重なり描画 + IME 位置ズレ + 追加要件 3〜5）
- 重なりの実測根因は「claude が空欄でも dim の案内文（`Try "…"` / キュー滞留時の
  `Press up to edit queued messages`）を箱の中へ描く」のに、tako が `has_text` だけを見て
  自前プレースホルダを同座標へ重ねていたこと。判定を `input_box_has_content` へ替えて根治
  （「上に N 行」の absolute 重ねも行として箱の上へ出す）
- IME 位置ズレはチャット表示がターミナルグリッドを描かないのにセル座標を
  アンカーにしていたため。ミラー行の実 bounds + TUI カーソルセル（`input_caret_cell`）から
  キャレット矩形を作り、未確定はその位置へインライン描画・候補ウィンドウも同じ矩形へ。
  未確定の見た目は `ime_preedit_text` の 1 実装をターミナル経路と共有
- 追加要件 3 = 作業中インジケータを会話末尾の AI 側へ / 4 = assistant にも枠 /
  5 = busy 中の指示は transcript の `queue-operation`（enqueue）を読んで即吹き出し化。
  **Issue の推定（system-reminder 過剰フィルタ）は実 transcript 3416 本の全数走査で棄却**
- 検証: 品質ゲート全緑（1801）+ 隔離セルフテスト項目 100 新設で完走 + visual-test 新節
  （dark/light の枠 + インジケータ差分）+ **実 claude e2e**（95c 拡張: 実スピナー行
  `Brewing… (running stop hook · 2s · ↓ 38 tokens)` 採取 / 配送前の吹き出し / 配送後 1 個）。
  検出力は 5 通りの revert で FAILED を実測
- 実 IME は**この開発機に日本語入力ソースが無く未検証**（manual-checks へ項目化）

## 2026-08-04（#745 / #746: チャットビューの md テーブル崩れと画像つき発話の二重表示）
- #745 の根因は**推定と別**だった: 「幅の伝播」ではなく assistant 発話の本文コンテナに
  付いていた `min_w(0)`。縦並びの子では自動最小サイズが高さ側に掛かるので意味を持たないのに、
  taffy の採寸経路が変わって表セルが折り返し幅 0 でレイアウトされたまま残る（同一幅 478px で
  セル「入力欄のテキスト重なり」が w=0.0/h=214.5 → w=143.0/h=19.5 = プレビューと同値）
- #746 の根因も**推定と別**: enqueue↔user 行の文面は実 transcript 17 件で一致しており正常。
  真犯人は楽観 echo が**生の TUI 入力行**（`[Image #13] …`）を持っていたこと。transcript と
  同じ分類器（`classify_user_content`）へ通して正規化し、重複排除も 1 実装（`echo_superseded`）へ
- 検証: 品質ゲート全緑（1806）+ 隔離セルフテスト完走（#746 項目新設）+ visual-test 全節完走
  （`chat-table` 節新設 = 同じ md をチャット / プレビューへ同じ幅で並べて実測）+
  実 claude e2e（95c 拡張: ⌘V で実画像 → `raw_line="[Image #1] …"` → 吹き出し 1 個 / queued=false）。
  検出力は #745 = visual-test が collapsed=2 で FAILED、#746 = ユニット 3 本 + セルフテスト項目で実証

## 2026-08-04（#749: master の自動ハンドオフ）
- ctx 閾値を 50〜60 で設定可能化（プロファイル → config.yaml → 既定 60。範囲外は明示指定は
  エラー / 手書きは丸めて warnings）+ tako が 2 秒 tick で閾値超過を検知して master へ
  引き継ぎを促す（画面由来の ctx% なので追加ポーリングゼロ）+ handoff の後任プロンプトへ
  「実態突き合わせ → 旧ペインの入力欄確認 → close」の順序つき手順を埋め込み（前任を
  閉じるのは後任だけ = 後任の起動失敗で master を失わない）+ master prompt に自動発動規範
- 判定と文面は `tako-core::handoff` の純関数。MCP ツールは増やさず
  `tako_orchestrator_profiles` に `ctx_threshold` / `auto_handoff` を載せた（CLI / GUI も同経路）
- 次: 実 claude の通し e2e（項目 101c）の実測を Issue にコメント

## 2026-08-04（#748: worker の選択肢ダイアログ対応の総点検）
- ダイアログの**存在判定を構造検知へ一般化**（`tako-core::dialog`。番号つき + 番号なしの 2 経路、
  罫線で挟まれた入力ボックスは棄却）+ 種別分類は文言（`claude_tui::DialogKind`: permission /
  trust / bypass / usage_limit / plan_confirm / select）。実採取は permission / `/model` /
  plan 確認 / `/mcp`（番号なし）/ AskUserQuestion（全角・5 択）
- 公開: `worker_status` / `read` の `choice_dialog`（ダイアログ中は `input_status` を null）、
  watch の `WORKER_DIALOG`（種別つき。question は同時に出さない）、respond の一般化
  （番号 / ラベル / `--choice` 省略で下見。**番号キーだけで確定** = 実測、番号なしは矢印 +
  ラベル一致検証 + Enter）、**ダイアログ中の send は選択肢つきエラーで拒否**、
  supervisor の limit 復旧を盲目 Enter → 安全な選択肢のラベル選択へ
- 検証: 品質ゲート全緑（1838）+ 実 claude e2e 2 本（permission の構造取得と番号キー確定 /
  `/model` を select として検知）+ セルフテスト項目 101 新設。limit ダイアログは実文言
  （バイナリ由来）+ 実レイアウト（`/model`）の合成 fixture
- 次: 隔離セルフテストの完走は環境負荷（load 25〜50）で未達 = main でも別項目で落ちる

## 2026-08-04（#750: MCP 133 ツール全体リファクタ）
- Issue に棚卸し・計画を記録し、PR #752 で完全カタログスナップショットを先行導入。
- #748 / #749 merge 後、MCP を catalog / request / HTTP / tests / facade へ挙動不変で分割開始。
- 完全スナップショット diff ゼロ、全品質ゲート緑、隔離セルフテスト完走（`TAKO_APP_SELF_TEST_OK`）。

## 2026-08-05（#761: handoff 後任 master が worker 設定で起動し default 扱いになる問題）
- 後任の起動を CLI `tako master -<profile>` と同一経路（`build_master_cmd`）へ寄せ、
  `TAKO_ORCHESTRATOR_ROLE` を env 用語彙（`master:<p>`）に是正。role 生成は
  `tako_core::handoff` の `master_pane_role` / `master_role_env` に一本化し、
  受け側は `master_profile_of_any_role` で表示用 / env 用どちらの語彙も解けるようにした
- 副産物: 後任に master system prompt が一切付いていなかった（worker 用コマンド構築の
  ため）のも同時に解消。組み立てをペイン分割の前へ移し失敗時に空ペインを残さない
- 検証: 隔離アプリ + 偽 claude の実経路 e2e で before/after 実測（before は
  worker model / `orchestrator-master:st761` / prompt 無し / self が profile=default、
  after は master model / `master:st761` / marker-found / profile=st761）+
  単体 5 本 + セルフテスト項目 102 新設（各バグを戻すと FAILED を実測）

## 2026-08-06（#772: stale binary 検知がメインスレッドを毎 tick 400ms 専有する問題）
- 真因は「毎 tick × 対象ペインごと」の `find_claude_pid_for_backend`（1 回で
  `tmux list-panes -a` + `ps` の 2 プロセス）。採取を `ProcessSnapshot` で 1 回に束ね、
  走査を background へ出し、`should_rescan`（起動直後 / 指紋変化 / 対象増減 / 60 秒）で
  頻度を落とした。それ以外の tick は stat だけ（`which claude` も PATH 走査へ置換）
- 実測（隔離・6 worker ペイン）: `periodic_prep:stale_binary` p50 289〜323ms → 0ms、
  しきい値超え行 24 行/60s → 0 行、`ps` 起動 175 回/60s → 34 回（stale ぶんは約 144 → 2）
- 検証: セルフテスト項目 103 新設（偽 claude の版差し替えで検知 + 変化無し tick の省略）+
  単体 9 本 + 品質ゲート全緑

## 2026-08-06（#770: 「再起動でタブが消えた」の根因確定と、喪失の記録・復旧の是正）
- 根因は再起動ではなく **10:13:27 JST のタブ × close**（ペインログの `close:gui-tab` +
  sessions.yaml の last_seen + 12:24 の再起動時点で既に 6 タブ 10 ペインだった復元数で確定）
- 直したのは気づけない / 戻せない側: セッション kill・タブ close の発生源つき監査行を
  persist.log へ（FR-5.15）/ バックアップ回転を「セッションを持つペインが消える保存」へ拡張
  （FR-5.11。実機は 12→10 で素通りし bak が 16 日前だった）/ 隔離限定の SIGTERM→quit 検証口
- 検証: 隔離 e2e でプレビュー混在タブの quit → 再起動が喪失ゼロ、close の before/after で
  監査行と `.bak.1` の有無を実測。番犬 3 本 + 単体は違反注入で FAILED を確認
- **事故**: 検証に使った System Events の Cmd+Q がグローバル送出で本番 tako に着弾し終了させた。
  以後キーストローク送出は禁止、quit は pid 指定 SIGTERM、e2e は本番 pid 不変をアサートする
- 次: 別 Issue 化（GUI close が workers.yaml に closed を残さない / テストが本番 data dir を汚す）

## 2026-08-06（#778: 後続 send 失敗の prompt_undelivered 偽陽性を修正）
- PromptFlow を spawn 初回 / 後続 send に識別し、後続失敗が worker の初回送達状態を変更しないよう修正。#530 の初回ダイアログ未達検知は維持
- 関連コミット: `add1053` `[修正] 後続send失敗のprompt未達誤検知を防ぐ (#778)`。PR #780 merge、Issue クローズ、全 CI 緑、0.6.7 app 導入済み
- 検証: unit 2 本 + 隔離実経路項目 105（busy 後続 send timeout 後も Delivered）+ workspace test / fmt / clippy / self-test 全緑

## 2026-08-06（#781: IME 位置・選択座標のズレは stale claude バナーの会計漏れだった）
- 疑われていた #725 / #737（チャット）の回帰ではない（稼働は `ui_mode: terminal`、
  #737 以降の diff に座標変更なし）。根因は **stale claude バナー（#498）の 28px** が
  `pane_text_areas`（PTY 行数 / マウス座標変換 / IME アンカーの共通の正）の算術から
  漏れていたこと。#684 が正にしたのはコンテナで、ペイン内部の積み上げは対象外だった
- タイミングの実測: claude の symlink 更新 13:40（2.1.220→2.1.223）→ ユーザー報告 13:43:42。
  自己更新で全 master / worker ペインに一斉にバナーが出るため周期的に再発していた
- 修正: 高さ定数を描画と会計で共有 / 矩形を `pane_text_area_rect` の 1 か所へ集約 /
  実描画のテキスト領域を採取するプローブを追加（正には使わず観測と perf.log 自己申告のみ）
- 検証: セルフテスト項目 106 が修正前 `gap 28.0` で FAILED → 修正後 `gap 0.0` で OK。
  単体 7 本 + 番犬（直接の子の数）。会計を外すと FAILED になることを実測
- 次: 実 IME の見た目は未検証（日本語入力ソースが無い）= ユーザー実機確認後にクローズ

## 2026-08-06（#779: sleep guard の定期 `ps` 起動削減）
- sleep guard の子プロセス走査を backend・role・OSC 状態の変化時 + 60 秒保険に限定し、
  #772 の `ProcessSnapshot` を stale binary 検知と共有。アイドル実測は `ps` 34 回 / 約 75 秒 → 3 回 / 約 72 秒（約 91% 減）。
- 隔離実経路で worker 実行中の sleep assertion 保持と終了後の解除を確認。CPU は高負荷下でも約 0.6% で目標 10% 未満、残る 2 秒 tick に常時重い処理は観測されなかった。
- 関連コミット: `14bea3a`（PR #783 squash merge）。#781 へ rebase 後の macOS CI 緑 +
  隔離セルフテスト完走（`TAKO_APP_SELF_TEST_OK` / FAILED 0。#771 型フレークは load 10 前後で消えた）

## 2026-08-07（#782: 見えないペインの出力で全面再描画しない + 描画コストの実測）
- 根因は「単一 GPUI entity なのでどのペインの出力でもアプリ全体を再描画」。裏タブ 2 ペイン
  200 行/秒で 22.3% CPU（見た目は不変）→ 可視性ゲートで **2.6% / 再描画 1596→11 回**
- 端末イベントを zed と同じ 4ms 合流に。`perf_span` をメインスレッド限定にし、background の
  `pdf_rasterize` が「メインスレッド専有」と誤記録されていたのを是正。フレームコスト実測
  （末尾 canvas）と実測連動のレート上限は #680 が落ちるため取り下げ（切り分け済み）
- Zed 同条件比較（137x13・200 行/秒）: tako 14.6% / 9.0G instr vs Zed 1.3% / 0.14G instr。
  1 フレーム = 5.1M（固定 = 毎フレーム全再構築）+ 0.39M×行数、Zed は 0.16M。**受け入れ基準
  「Zed 同等」は未達** = 別 Issue（専用 Element 化 + ビュー単位キャッシュ）へ
- 関連コミット: `1994b8e`

## 2026-08-07（#786: クローム・ペインのビュー単位キャッシュ）
- ペイン本体（`PaneBody`）とクローム 4 枚（`Chrome`）を `AnyView::cached` の単位へ切り出し、
  PTY 出力はそのペインのビューだけを notify（それ以外は従来どおり全体 notify + `cx.observe`）
- 実測（隔離・200 行/秒・同一バイナリ A/B、表示中 2 ペイン）: 4 タブ 25.30% → 18.04% CPU /
  6.772M → 5.016M instr/frame、17 タブ 36.65% → 8.94% / 9.693M → 5.574M。
  クローム増分は 2.92M → 0.56M（−81%）= 固定費がほぼ消えた
- 踏み抜き: 親の render で `cx.new` したキャッシュビューは `tracked_entities` から落ちて
  二度と描き直されない（`cached_view` で毎フレーム `read` して固定）。
  副産物で #749 以降ビルド不能だった visual-test も復旧
- 関連コミット: `0188d79`（PR #788 squash merge）。macOS / Windows CI 緑、install 済み
- 次: #787（端末グリッドの専用 Element 化）で残りの 5M/frame を削る

## 2026-08-10（#496 残バグ: git パネルのクリックが一括 dismiss に食われる問題を根治）
- ルート div の `on_mouse_down` が呼ぶ `clear_text_input_focus()`（#503）が押下の瞬間に
  状態を落とし、コンフリクト解消エージェント 3 択の `on_click` が **merge 時から一度も
  発火していなかった**。同型 4 件（トグル / ブランチ名入力欄 / 作成 / キャンセル）も一括修正
- 実測: visual-test 新節 `conflict-card` で claude→pane2 / codex→pane3 / agy→pane4 が
  実マウスで立つ。guard を外すと `panes 1->1 / feedback=None` で FAILED（検出力）。
  CI 用に番犬テスト 3 本 + 規約を `.agent/conventions.md` へ明文化
- 同梱: セルフテスト #601 の固定待ちをリトライ化（main 由来の確定失敗）、visual-test の
  clippy 違反 2 件（#745 由来）
- 次: PR → CI 緑 → merge → install。PDF / IME / tmux のセルフテスト項目は main 由来失敗で別起票

## 2026-08-10（#793: setup に設定共有（tako config）の検出・案内・代行導線）
- `config_share::env` を新設し、①配線済みか ②共有対象が既に外部 git（dotfiles）で
  管理されていないか ③gh の認証状態、を読み取りだけで検出。案内は純粋関数 `guidance` で決め、
  setup サマリと `--check` が同じ判定から文言を作る（質問は増やさない / 配線済みなら勧誘しない）
- 代行はアシスタント側: `setup-context.yaml` の `config_share` + system-prompt Step 3.5。
  **既存 dotfiles があれば相乗りが第一案**（別リポジトリだと pull の rename が symlink を
  実ファイルへ置き換えて既存の配線を壊す）。既存ユーザーへは changes.yaml rev 14（guided）
- 検証: 隔離 e2e PASS 55 / FAIL 0（本番 HOME・~/.claude・GitHub 非干渉を含む）+
  品質ゲート全緑（1921）+ docs build

## 2026-08-11（#787 前提整備: 端末グリッドの visual-test 回帰検出網）
- visual-test に `terminal-grid` 節を新設（6 検査 = 日本語混在行 #64 / ピクセルスクロール #159 /
  選択ハイライト #725 / 端末 SGR の色・属性 / IME アンカー #781 / カーソル 4 通り）。
  Element 化本体（#787）は後続 worker
- 描画本体は無変更: 追加は全部 `#[cfg(feature = "visual-test")]` で、feature 無しビルドの
  シンボル 135,988 件と `__text` 49,141,068 バイトが main と完全一致
- 検証: 3 回連続で全緑（数値も完全一致）+ 全節通し 96 checkpoint 緑 + 検出力 3 件実証
  （nowrap 除去 → max 消失を検出 / subline シフト 0 → 位相 34px で検出 / テキスト領域
  会計を 8px ずらす → 最初の幾何検査で検出）+ fmt / clippy(feature 込み) / test 1924 緑
- 副産物: #797（SGR 4 の下線が 1 px も描かれない = 行ボックス下端で overflow_hidden に切られる）/
  #798（全角の長い連なりで描画位置が最大 1 セル左へ詰まる）を起票。#796 へ
  「visual-test feature 付きビルドではセルフテスト #600 が確定失敗（main も同じ）」を追記
- 次: #787 本体の worker が before/after をこの節で突き合わせる

## 2026-08-11（#787: 端末グリッドの専用 Element 化 + #797 / #798 の根治）
- ペイン本体のグリッドを行 div スタックから 1 個の `Element`（`terminal_grid.rs`）へ。
  セル原点を `col * cell_width` で直接決め、背景 = `paint_quad` / グリフ =
  `shape_line(force_width)` / 下線・取り消し線 = 自前。**全角の 2 セル目にスペースを
  差し込んで** force_width のセル境界スナップを全角行でも効かせる
- 副産物ではなく設計上の帰結として #797（SGR 4 と ⌘ホバーの下線が 1 px も出ない =
  行 div の overflow_hidden に切られていた）と #798（全角の長い連なりで最大 1 セル
  左へ詰まる = div 幅のデバイスピクセル丸めの累積）が解消。行高をセル高にしたので
  ディセンダの切れも直った（**字が 2px 上へ動く = 目に見える変化**）
- 性能（同一バイナリ A/B。`TAKO_787_NO_GRID_ELEMENT=1` が before・300 フレーム）:
  実務密度 15.68M → **6.42M** instr/frame（−59%）/ 満杯 15.59M → **8.68M**（−44%）。
  グリッド分だけなら 0.520M/行 → **0.079M/行**（実務密度）
- 検証: 品質ゲート全緑（1935）+ Windows クロスチェック警告 16 = main 同数 +
  visual-test `terminal-grid` 3 連続 OK（新設 4 検査は旧経路で落ちることを実測）+
  隔離セルフテスト完走 + 実 claude 13 行 523 セルで missing 0 / drift 0。
  全節の PDF 失敗は素の main でも再現 = main 由来（#796）

## 2026-08-13（#801: 描画の残る固定費の内訳確定 + セル単位の変換の削減）
- #787 後に残った「空画面でも毎フレーム 4.76M instr」の内訳を段階的無効化ゲートで確定:
  ウェルカムバナー 1.17M（初回起動のみ）/ スナップショット + `plan_row` 1.76M /
  ペインヘッダ 0.81M / クローム再利用 0.46M / ルートの箱 0.41M / gpui 下限 0.16M
- 支配項（セル単位の変換）を削減: 素の空白セルは解決も書き込みもしない・空行は
  `compose_line` を 1 本だけ組んで複製・`plan_row` は空行を即返し `Rgb->Hsla` をラン単位へ。
  空画面 3.587M → **2.197M（−39%）**、実務密度 −10%、満杯 −2%（同一バイナリ A/B）
- **目標の 1M 未満は未達**。ヘッダ 0.81M は「`AnyView::cached` は入れ子にできない」
  （GPUI が再描画中 `refreshing` を立てる）で塞がっており、取るにはペイン枠ごと
  ルート側へ持ち上げる必要がある。実測と回避案は architecture.md に記録
- 検証: 品質ゲート全緑（1944）+ visual-test 全節 3 連続 OK + 隔離セルフテスト完走 +
  隔離実操作 12/12（出力・テーマ・分割・フォーカス・スクロール）

## 2026-08-14（#792: handoff を「知識（マシン非依存）」と「実行状態（このマシン限定）」に分離）
- 書式の正本を `tako_core::handoff` に新設（見出し 4 定数 + 寛容な `section_of_line` +
  `split_handoff` + `handoff_template`）。後任プロンプトは**全文をそのまま渡した上で**
  節ごとの扱い（知識 = 前提にしてよい / 実行状態 = 必ず実態で確認）を添える
- **旧書式はそのまま動く**: 節が無ければ Legacy として全文を渡し「番号は実態で確認 +
  次の更新で 2 節へ書き直せ」を添えて自然な移行に任せる（一括変換はしない）。
  本番の実 handoff 5 本を読み取りだけで実測 = 全部 legacy / 全文保持
- master prompt の規範を新書式へ改訂（見出し定数とのドリフトはテストが落とす）。
  応答に `handoff_format` / `handoff_sections` を追加（self / handoff の両方）。
  solo は handoff 機構を持たないので規範なし（プレースホルダ置換だけ）= 変更不要
- ついで: `_system_prompt_*` を Local(GENERATED) でカタログ登録し、被覆テストの走査を
  `join(format!(…))` へ拡張（before: `unclassified` に 2 件 → after: 0 件を実バイナリで A/B）
- 検証: 品質ゲート全緑（1966）+ Windows クロスチェック警告 16 = main 同数 +
  隔離セルフテスト完走（項目 102b 新設 = 実 dispatch で sectioned / legacy を実測）+
  検出力 3 件（カタログ削除で 2 テスト FAILED / prompt 見出しドリフトで FAILED /
  節判定の破壊で 8 テスト FAILED）
- 関連コミット: `40c4b2a`（PR #804 squash merge）。CI macOS / Windows / Pages 全緑 +
  `/Applications/tako.app` install 済み（反映は再起動後）。証拠は ~/dev/tako-evidence/792/

## 2026-08-14（#789: サイドバー幅のクランプ規則を全経路で統一）
- 上限がドラッグ = ウィンドウ幅の 50% / dispatch = 固定 600px で食い違っていた（#307 の
  クローズ検証で発覚）。規則を `tako_core::sidebar`（下限 120 / 上限 = ビューポート幅の 50%）へ
  一本化。**ドラッグ側へ寄せた**理由は ①固定 px では広い窓で CLI がドラッグ相当の幅に
  届かない（設計原則 5 の破れ）②固定 px は狭い窓で過大（600px は 800px 窓の 75%）
- 状態は要求値・描画は実効幅（`effective_sidebar_width`）に分離。窓が狭くなっても要求値は
  書き換えないので広げ直し / 再起動で元の幅へ戻る。dispatch はウィンドウを持たないので
  上限を最後に描いたビューポート幅から取り、応答に `sidebar_width_max` / `_min` を追加。
  永続化も要求値 → 適用値へ（settings.json と画面の食い違いを解消）
- 検証: unit 7 本 + セルフテスト項目 109（実ハンドラ `on_mouse_move` と実 dispatch へ同じ値を
  入れ、窓 1600 = 上限 800 で一致を見るので旧固定 600 は必ず落ちる。窓 700 への縮小 →
  再拡大も含む）。旧挙動 2 通りへ戻すと項目 109 が FAILED になることを実測。品質ゲート全緑（1951）
- 副産物: Metal Toolchain（purgeable 資産）がマシンから消えており全 worktree で gpui の
  シェーダをビルドできない状態だったので `xcodebuild -downloadComponent MetalToolchain` で復旧

## 2026-08-14（#790: worker への指示送達を Cross-Session Messaging 優先の二層へ）
- claude v2.1.224+ の受信箱（socket 直送）を第 1 層に、従来のキー操作経路を第 2 層に。
  適用は worker 宛のみ（受信側に抑制不可の前置きが付くため人間由来の送達は従来経路）
- 実測: 実験フラグ不要（サーバー側 gate 依存）/ idle・busy・ダイアログ中とも送達成立 /
  43,449 バイトをバイト等価に 1 回で送達。前置きの存在と挙動への影響は Issue に記録
- 検証: 実 claude e2e 3 本（idle=peer/delivered / busy=送信時点 busy で peer/queued /
  peer off でキー経路 verified）+ 既存 `claude_tui_e2e` 6/7（残り 1 は main でも同一に失敗）+
  品質ゲート全緑（1990）+ Windows クロスチェック警告 16 = main 同数 + 隔離セルフテスト完走
- 関連コミット: `f57e661`（PR #806 squash merge）。install 済み（反映は再起動後）。
  副産物 #807 起票（`ui_text::update` の言語グローバル競合フレーク = #608 の取りこぼし）

## 2026-08-15（#658: worker レジストリの残留と GC 不全を main へ移植）
- #658 は 2026-07-31 に「クローズ済み」だったが、PR #701 の base は
  `windows/467-ipc-orchestration-local` で **main には 1 行も入っていなかった**
  （`dead_since` が存在しない）。本番 workers.yaml も 51/53/54/184 が active のまま・
  `dead_since` 未刻印で症状継続。再実装ではなく `ef89ca3` を main へ移植した
- 中身は 3 層: ①セルフテストの隔離対象を `self_test_isolation_defaults()` へ集約
  （`TAKO_WORKERS_FILE` / 新設 `TAKO_ORCHESTRATOR_DIR`）+ 項目 0 で実プロセス検査
  ②GUI 経路の close をレジストリへ記録（main は `CloseReason::Explicit(CloseOrigin)`
  なので `is_explicit()` へ適応）③`workers` 列挙のついでの GC（ペインも器も見えない
  active に `dead_since` を刻み、**300 秒続いたものだけ** closed(gone)）。仕様は
  requirements.md に **FR-2.26** を新設（#390 は FR が無かった）
- 検証: 品質ゲート全緑（fmt / clippy -D warnings / test。#658 の unit 8 本）+
  隔離 GUI + 本番コピーのレジストリで**実時間 310 秒待ちの通し**（1 回目 = 14 件に
  `dead_since` 刻印・closed 0 / 2 回目 = 14 件 closed(gone)、生きたペインを指す
  エントリは active のまま・刻印もされない）+ closed 後も `resume_command` が
  引けること（claude worker 10 件）+ 隔離漏れの陰性対照（項目 0 が exit 1）
- 副産物: **tako ペインの中から CLI を叩くと `TAKO_SOCKET`/`TAKO_TOKEN` が本番 GUI を
  指す**ため、data_dir / discovery を隔離しても本番へ届く（1 回踏んだ。本番 GUI が
  旧バイナリ = sweep 非搭載で実害ゼロ）。隔離検証は `env -u TAKO_SOCKET -u TAKO_TOKEN` 必須
- 次: 本番の掃除は install + GUI 再起動後に `tako orchestrator workers` を 2 回
  （5 分あけて）。GC は GUI プロセス側で走るので旧バイナリのままでは倒れない

## 2026-08-14（#796: 隔離セルフテストの main 由来フレークを根治）
- 根因 3 つを実測で確定: ①**#786 の `AnyView::cached` と「汚さずに draw」**（製品経路は
  dispatch 後に `cx.notify()` するのにセルフテストはしていなかった → 幾何がキャッシュのまま
  = PDF アウトライン #232 が「ジャンプが効かない」に見えていた。実測 `children=2` /
  `max_offset_y=199` なのに `offset_y` が 4 秒 80 フレームで 0 のまま。同機序で #702 の
  下端追従も）②**偽の待ち条件**（#601 の A / B 両フェーズが同じ `ST601>`。旧形式で
  実測 `shared_prompt=Some(0)` = 起動前に待ちが成立）③**「出るもの」を固定時間で待っていた**
  26 組（`--features visual-test` は gpui の leak-detection を有効にして数割遅い）
- 実装: `wait_for_focused_text` / `_timed` / `absent_after_anchor` / `notify_and_draw` /
  `PdfScrollProbe` / `TAKO_APP_SELF_TEST_ENV`（profile / feature / load / 経過。load は
  `tako_control::diag::load_average` 新設）/ #732 の前提待ち / 番犬テスト
  `selftest_wait_watchdog` / 規約を conventions.md へ明文化
- 検証: 人工負荷（`yes` 6 本・load 14〜74）で feature 無し 5 回連続 OK + feature 付き 3 回連続 OK
  （feature 付きは修正前 4/4 で `PDFKit アウトライン…` に確定失敗していた）。品質ゲート全緑
- 副産物: このマシンの **Metal Toolchain 不在**でセッション前半は gpui のシェーダを
  ビルドできず（`xcodebuild -downloadComponent MetalToolchain` で復旧。23:00 に解消）

## 2026-08-15（#803: ペインヘッダをルート側の兄弟へ持ち上げてキャッシュ単位にした）
- ペインのタイトルバーは PTY 出力では変わらないのに毎フレーム作り直していた（`AnyView::cached`
  は入れ子にできないので本体の内側では一度も当たらない。#801 の実測）。`view_cache::PaneHeader`
  として本体の**兄弟**へ出し、本体は同じ高さのスペーサーで場所を空ける（`pane_text_areas` の
  会計は不変）。ヘッダを出すかは「本体が実際に場所を空けたか」で決める = 表示種別が変わった
  フレームで二重に出ない。`running · 4m12s` の時計だけ 1 秒に 1 回別枠で汚す
- 実測（隔離 grid-bench 300 フレーム・main と交互 3 反復の中央値）: 空画面 2.192M →
  **1.737M（−21%）**/ 実務密度 5.158M → 4.698M / 満杯 8.435M → 7.977M。ヘッダを丸ごと
  描かないゲート（1.517M）との差から**ヘッダ総コスト 0.678M のうち 0.455M（67%）を回収**。
  残り 0.22M は `cached` の再利用そのもの（Issue の 0.81M は #801 の別構成での値）
- 踏み抜いた罠: GPUI は「影 → 背景 → 子 → **枠線**」の順に塗るので、兄弟にしただけだと
  ペイン枠の上 2 つの丸め角がヘッダの四角い背景で潰れた（実フレーム比較で accent が
  104 画素消えているのを検出）。外側 div にも同じ矩形・同じ色の枠線を描かせて重なり順を戻した
- 検証: 品質ゲート全緑（1974 tests / fmt / clippy(feature 有無) / Windows クロスチェック
  エラー 0・警告 16）+ 隔離セルフテスト完走（項目 110 新設。`output=(body +1 header +0)` /
  `title=(body +2 header +2)` / 実描画矩形の差 0.0px）+ visual-test 全節 3 連続緑 +
  **全 98 ピクセル計測値が main と一致**（差は md の load ms のみ）+ 実フレーム
  2200x1416 の全画素比較で差は 32 画素（0.001%。角の枠線 AA の二重合成のみ）
- 検出力: ヘッダを本体の内側へ戻すと項目 110 と番犬テスト 2 本が落ちることを実測

## 2026-08-15（v0.7.0 安定版リリース）
- v0.6.0 以来の安定版。CHANGELOG に `[0.7.0]` を新設し、夜間 v0.6.1〜v0.6.11 + 化石化した
  `[Unreleased]` 2 ブロック + v0.6.11 以降 11 コミットを日英併記で統合（柱 = GUI ライク表示
  モード / 描画コスト削減 / AI 連携）。実質コミット 56 件の Issue 番号が節に現れることを機械確認（未カバー 0）
- tag `v0.7.0` + GitHub Release を **Latest（安定版）**で公開、cask 0.7.0（sha256 は brew fetch で検証）、
  `/Applications` = 0.7.0。CI macOS / Windows 緑、夜間は `SKIP: 変更なし` へ復帰
- 関連コミット: `20ef2a1`（tako）/ `2651bf2`（homebrew-tako）、tag `v0.7.0`
- 次: ユーザーの GUI 再起動で反映。docs サイトのライブ反映は未確認（デプロイ先 URL がリポジトリに無い）

## 2026-08-15（#813: 利用上限後のペイン単位の自動復帰）
- ペイン単位のオプトイン（右クリック / `tako limit-resume` / MCP `tako_limit_resume` の 3 経路が
  同じ dispatch）で、5h / 週次上限で止まったエージェントをリセット時刻 + 安全マージン後に再開させる。
  ダイアログ型は「解除まで待つ」をラベル一致で確定（課金・モデル変更は拒否リストで構造的に排除）、
  idle 型は継続ナッジを送達確認つき経路へ。FR-2.27 新設
- 層は 3 つ: `tako_core::limit_resume`（純関数の判断・時刻パース・選択肢選別）/
  `tako_control::limit_stop`（#748 と #157 の既存検知を束ねるだけ）/ `tako-app::limit_autoresume`
  （2 秒 tick。有効ペイン 0 なら即 return）。supervisor（#401）の `safe_limit_choice` も core へ寄せた
- 検証: 品質ゲート全緑 + 隔離セルフテスト完走（項目 111 新設 = 正例 2 型 / 負例 3 型 /
  試行上限 / list・read の一致）+ visual-test 全節（98 checkpoint）+ Windows クロスチェック
  （エラー 0 / 警告 16 = main 同数）
- 次: PR #820（Closes #813）→ macOS CI → squash merge → `build-app.sh --install`
## 2026-08-15（#815: 構文セットを「使っている間だけ」載せる）
- Issue の前提を計測が覆した: `SyntaxSet` の器は **1.04 MB / 1.2 ms** しかなく、98 MB の正体は
  **ハイライトした言語ごとの遅延展開**（Rust +5.1 / bash +10.9 / md +10.9 / TS +32.0 MB。18 言語で 149 MB）。
  syntect に言語単位で捨てる API が無いので、推奨案の段階ロードは器の 0.64 MB しか減らず
  未対応 363 拡張子の回帰リスクだけが残る → 棄却し、借用チケット（`SyntaxLease`）+
  無使用の解放（猶予 30 秒 / プレビュー 0 枚なら即）にした
- 効果（隔離・同一バイナリ A/B。`TAKO_815_NO_SYNTAX_RELEASE=1` が旧挙動）: 小 .rs + 小 .ts を
  2 枚開いて **83.57 MB → 13.04 MB（起動直後 13.06 MB へ完全復帰。−70.5 MB）**。
  開いたままでも 40 秒で 83.55 → 13.05 MB（before は不変）
- 検証: 品質ゲート全緑（fmt / clippy -D warnings / test 1997）+ 単体 6 本（ローカル
  `SyntaxCache` で並列テストでも決定的）+ 拡張子全数解決テスト + セルフテスト項目 112 新設
- 副産物（別 Issue へ）: **大きいファイルのプレビューは行数に比例したヒープが残る**。
  3629 行で 12.6 → 183.1 MB、閉じても 162.7 MB（150 MB 残留）。開閉 3 往復で積み増し。
  10 行なら完全に戻るので行数依存 = syntect ではない（A/B の両側で残る）

## 2026-08-15（#817: PTY reader の 1 MiB スタックバッファを根治）
- alacritty_terminal の `EventLoop` をやめ、同等のループを tako が持つ形へ
  （`tako-core::pty_loop`。upstream は reader スレッドのスタックへ 1 MiB を置き、ゼロ初期化で
  ペイン 1 枚 = 約 1.03 MB が常駐していた）。定数は `pub(crate)` で下げられず、**スレッドの
  スタックサイズを絞っても footprint は減らない**（resident なのは memset された分）ため、
  バッファをヒープへ動かすにはループを持つしかなかった
- 読み取りバッファは 64 KiB 始まりで、ロック競合で足りないときだけ 1 MiB まで伸ばして戻す。
  ロック粒度（`MAX_LOCKED_READ`）・上限到達時のブロッキングロック・シャットダウン順序は upstream のまま
- 実測（隔離 release・16 ペイン）: stack **17 MB → 848 KB**（1 MB 級スレッドスタック 16 本 → 0 本）、
  phys_footprint **226 MB → 211 MB（−15 MB）**。MALLOC_SMALL は +1 MB（16 × 64 KiB）
- CPU 悪化なし。裏タブへ流して取り込み経路だけを測り、固定仕事量 0.41/0.41/0.39 → 0.41/0.40/0.39 CPU 秒、
  実行数で正規化した 200 行/秒は 3 ペアとも after が低い（149.1/116.3/122.9 → 144.9/105.1/117.7 cpu_ms/1000 行）
- 挙動: 4 ペイン洪水完走・26 KB 貼り付けの往復がバイト等価（md5 一致）・seq 50000 の末尾連続・
  洪水後も CLI 応答。Unix の poller トークンは実 PTY を張る単体テストで検出（壊すとハングでなく FAILED）
- 関連コミット: PR（Closes #817 / Refs #814）。由来と改変は `THIRD-PARTY-NOTICES.md` へ追記

## 2026-08-15（#821: コードプレビューの行数比例リークを仮想化で根治）
- 根因は allocation プロファイルで確定: 全行ぶんの element を毎フレーム作るため、
  1 フレームぶんの測定レイアウトノードが taffy の `node_context_data` に残り続ける
  （`TaffyTree::clear()` がこれを消さない）+ アリーナ / フレーム Vec の高水位。
  `gpui::list` で可視行だけ描く形へ変え、閉じたあとの残留 110.1 MB → 2.2 MB（1 万行は
  footprint 210 → 46 MB）。見た目は旧経路と実ピクセル差 0（visual-test `preview-code` 節を新設）
- 同梱: CLI / MCP の close がプレビュー状態を落としていなかった実バグを
  `drop_preview_pane_state` への集約 + 番犬テストで根治
- 事故: 後始末の `pkill -x tako-app` が本番 GUI にも当たり終了させた。再起動で
  9 タブ 21 ペイン完全復元。以後、隔離インスタンスは明示 pid でのみ落とす

## 2026-08-15（#816: 取り込み経路の CPU — 層別内訳の確定と支配項の削減）
- 計装ビルド（シンボル付き release + env ゲート）で層別に割ると、支配項は**パースではなく
  イベント配送**（35.8%）だった。しかも配送コストは行数ではなく **PTY read の回数**に比例し、
  同じ 6000 行でも「20 行バースト」353M に対し「1 行ずつ」は **1565M（4.4 倍）**
- 直したのは 5 つ: ①見えないペインの Wakeup はメインスレッドへ渡さない（`PaneDelivery`。
  #782 の可視性ゲートは渡った後で効くので往復ぶんは払っていた）②見えていても再描画間隔
  （16ms）より細かく往復しない ③未処理 Wakeup がある間は PTY 側が次を送らない
  （`wakeup_gate`。受け手主導のバックプレッシャ）④ペインログの行取り込みの二重確保と
  末尾空白走査を除去 ⑤OSC tap は `Ground` を次の ESC まで飛ばす
- 効果（隔離・交互 3 反復の中央値・instructions）: 裏タブ + 1 行ずつ = **1569.4M → 686.7M
  （−56.2%）**、表 + 1 行ずつ −16.6%、20 行バースト −5.5%（表）/ −11.4%（裏）。
  **エージェント worker が最大の受益者**
- **#814 の前提の訂正**: 「裏タブでも残る 9.28% が取り込み経路」は再現せず実測 0.2〜0.3%。
  この機は GPUI がウィンドウを 1 フレームも描いていない（セルフテストが自己申告）ため、
  9.28% は描画込みの値だった
- 挙動不変を before/after で突き合わせ: 4 ペイン洪水 4/4・seq 末尾連続 4/4・26KB 往復 md5 一致・
  OSC 7/133 検知一致・**ペインログ md5 完全一致**・洪水直後の list 応答 18/19ms
- 検証: fmt / clippy(-D warnings) / test --workspace 全緑 + Windows クロスチェック警告 16
  （main 同数）+ 隔離セルフテスト `TAKO_APP_SELF_TEST_OK`（項目 113 新設）+
  visual-test 全 98 checkpoint を 3 回連続で緑
- 事故: `pgrep` 空振りで `TAKO_SOCKET` が空になったワンライナーが CLI 既定の接続解決を通って
  **本番 GUI にタブを 1 枚作った**（即 close で復旧、他の状態は無傷）

## 2026-08-15（#826: Markdown プレビューの行数比例リークを仮想化で根治）
- #821 と同じ機序（全ブロックの element を毎フレーム作る → 1 フレームぶんの測定
  レイアウトノードが taffy の `node_context_data` に居座る）が md にも残っていた。
  `gpui::list` で**可視ブロックだけ**を組む形へ（**1 item = 1 ブロック** = #232 の
  目次ジャンプの対応をそのまま保つ）。器はコードと共用の 1 本にして、モード切替で
  「行番号とブロック番号を取り違える」事故を種別つきの `ListState` で構造排除
- 行テキスト・行頭オフセット・ブロック索引は**文書全体ぶん**作る（element は増えない）
  ので、⌘A / コピー / ヒットテスト / リンク索引（#680）は描画状態に依存しない。
  目次ジャンプは `ListState::scroll_to` にしたので**一度も描かれていないブロック**へも届く
- 実測（隔離・同一バイナリ A/B。1,819 ブロック）: 開いた 90.23 → **24.29 MB**、
  閉じた残留 67.10 → **1.86 MB**（3 往復でも 2.05 MB で収束）、整形した行 3,408 → **7**、
  定常フレーム 0.63 → **0.11 ms**、peak RSS 192 → **115 MB**。実文書
  （progress.md = 1,014 ブロック）でも残留 31.1 → 1.60 MB
- 見た目: **同じスクロール位置のフレームは実ピクセル差 0**（dark/light/narrow の文書先頭）。
  visual-test 98 checkpoint のうち main と違うのは md 節の 11 行だけで、
  中身は「記録した行レイアウト数」「掃引の指標」「ロード ms」= 計測の意味が変わったぶん
- 同梱: `remove_tab_with` の独自列挙を `drop_preview_pane_state` へ集約（番犬の走査が
  `&pane` 決め打ちで `&id` のループを見逃していた）/ visual-test の md・PDF が
  `drain_pending_preview_loads` を呼ばず「たまたま通っていた」main 由来のフレークを固定
- 検証: fmt / clippy(-D warnings) / test --workspace 全緑 + Windows クロスチェック
  エラー 0・警告 13（main 同数）+ 隔離セルフテスト `TAKO_APP_SELF_TEST_OK`（項目 114 新設。
  旧経路では `shaped=608`（= 全行）で FAILED になることを実測）+ visual-test 全 98
  checkpoint を 3 回連続で緑

## 2026-08-15（#826 追随: ライブリロードの位置保持検査が空振りしていたのを直す）
- #826 で md 本文の器を `list` にしたことで、セルフテスト 66c（#233）の
  「スクロールハンドルへ offset を書く → 同じ値を読む」が**画面に触れない空振り**に
  なっていた（`TAKO_APP_SELF_TEST_OK` でも中身を検証していない状態）
- 器に合わせて位置を指し・読むように是正（`preview_md_list` があれば
  `logical_scroll_top` を 1 本の実数にして比べる）+ 「読む前に 1 フレーム描く」
  + 書き換えごとにブロック数を変える（同数だと仮想リストが作り直されず、
  位置を持ち越す経路そのものを通らない）
- 検出力: `preview_body_list_state` の持ち越しを外すと
  「ライブリロード後もモードとスクロール位置を保持」が FAILED になることを実測
  （直す前は外しても通っていた）。`scroll_mark=8.0000->8.0000`（ブロック数 101 → 107）
- 検証: fmt / clippy(-D warnings) / test --workspace 全緑 + 隔離セルフテスト
  `TAKO_APP_SELF_TEST_OK`

## 2026-08-15（#828: window close の残留は gpui / AppKit 層。診断だけ足した）
- Issue の目星（`sync_viewports` の Err 握り潰し）を計装で反証: `handle.update` は毎回 `Ok`、
  `MacWindow::drop` も走り（`delegate=nil` / `isVisible=false`）、残るのは NSWindow が
  解放されないこと（`retainCount` 24→8 で不変）だけ。**素の gpui でも赤ボタン相当の
  AppKit 起点 close でも同じ**で、`leaks` も到達不能リークを報告しない
- 実装は最小のハードニングのみ: close 失敗を発生源つき（`render` / `dispatch` / `selftest`）で
  persist.log へ記録。再試行はせず**挙動不変**。番犬テスト 3 本（握り潰しへ戻すと FAILED を実測）
- 蓋を開けた状態での再計測は未実施（この機は clamshell 閉・画面 OFF で全面黒しか撮れない）。
  証拠と再現ハーネスは `~/dev/tako-evidence/828/`

## 2026-08-15（#830: チャットビューの行数比例リークを仮想化で根治）
- #821 / #826 と同じ機序（1 フレームで作った element の数だけ taffy の
  `node_context_data` に測定ノードが残り `clear()` で消えない）がチャットにも残っていた。
  会話本文を `gpui::list` にして**可視の発話だけ**組む形へ（1 item = 1 発話 + 末尾の
  付随要素 = カード / 作業中 / 承認）。行テキストは**文書全体ぶん**作るので ⌘A・コピー・
  ヒットテストは描画状態に依存しない
- **効くのは会話の長さではなく 1 タブに何枚あるか**（`CHAT_TAIL` = 50 で会話は頭打ちだが、
  表示中のペインは全部 element を作る = master + worker が同居する tako の実運用が直撃）。
  実 transcript（tail 50 = 534 md ブロック）の残留: 1 枚 11.32 → **2.53 MB** /
  4 枚 43.78 → **4.10 MB** / 8 枚 86.24 → **8.18 MB**。整形した行 580/2,320/4,640 →
  26/104/208（索引の行数は不変）。定常フレームは 8 枚で 0.68 → 0.41 ms
- 唯一の挙動差: チャットを開いた**最初のフレームで末尾**が出る（旧経路は
  `ScrollHandle::scroll_to_bottom()` が前フレームの実測に依存し初回は動かず、tail 50 の
  いちばん古い発話から表示されていた = 承認カードが出ても画面外だった）。
  「既定は追従」の設計意図どおりなので直した側を採用し、architecture.md に明記
- 検証: fmt / clippy(-D warnings、visual-test feature 有無とも) / test --workspace 全緑 +
  Windows クロスチェック エラー 0・警告 16（main 同数）+ 隔離セルフテスト
  `TAKO_APP_SELF_TEST_OK`（項目 115 新設。旧経路では `shaped=321`（= 全行）で FAILED に
  なることを実測）+ visual-test 全 98 checkpoint を 3 回連続で緑（`chat-table` は
  3 状態とも 1 文字も変わらない）
- 同梱: 計測ハーネス `TAKO_VISUAL_ONLY=chat-leak`（実 transcript / 枚数を指定できる）、
  項目 98 のヒットテストを「見えている行を掴む」形へ是正、`--features visual-test` 時の
  clippy 違反 1 件（main 由来）を修正

## 2026-08-17（#835: Finder の「このアプリケーションで開く」で新しいタブが開く）
- #708 は受け口まで作ってあったが開く先が**アクティブタブのプレビュー再利用**で、複数選択
  すると最後の 1 枚しか残らず「選んでも何も起きない」に見えていた（旧挙動へ戻すと
  セルフテスト 116 が `tabs 3->4` / `new=[("プロジェクト", 2, Some("…/unknown.xyzzy"), true)]`
  = 3 ファイルが 1 ペインに潰れることを実測）。**新しいタブ**で開く形へ是正
- 振り分け: ファイル（宣言外の形式も）= プレビュー 1 枚だけのタブ（PTY なし・タブ名 =
  ファイル名の手動タイトル）/ フォルダ = そのフォルダでシェルを起動したタブ / 不在パスは
  読み飛ばし。複数選択は **1 ファイル = 1 タブ**（最後が前に出る）。既存タブは不変
- 新ツールは作らず既存 dispatch を 2 本拡張: `OpenFile { new_tab }`（`tako open --new-tab` /
  MCP `new_tab`。`direction` とは排他）と `TabNew { cwd }`（`tako tab new --cwd` / MCP `cwd`。
  存在しない・フォルダでないパスは起動前にエラー）。MCP ツール数は不変
- 検証: 品質ゲート全緑（fmt / clippy -D warnings / test）+ Windows クロスチェック警告 16
  （main 同数）+ 隔離セルフテスト `TAKO_APP_SELF_TEST_OK`（項目 116 新設）+ **隔離 .app の
  `open -a` e2e 22/22**（bundle id を差し替えたコピー + LSEnvironment で隔離。cold launch で
  復元 3 タブ + 新規 1 タブ / 起動中 / 複数 / フォルダ / 宣言外 / 不在パス / CLI・MCP 1:1 /
  本番の pid・layout.json 不変）。検出力は 3 通りの revert で FAILED を実測
- 副観点: Finder に tako が 2 つ出るのは `~/dev/tako/dist/tako.app`（build-app.sh の生成物・
  .gitignore 済み）が LS へ自動登録されるため。掃除手順は Issue / PR に記載（自動掃除はしない）

## 2026-08-18（#838: Web ビューペインのちらつきを根治）
- 根因は**可視性の「印」方式が #786 で壊れていた**こと: ペイン本体が `AnyView::cached` の
  子ビューになり、キャッシュが当たったフレームは子の render が走らない = 印が付かない →
  ルートの掃き出しが webview を隠す → 次の `TakoApp` notify で再表示、の往復。
  #816 で PTY 出力が**そのペインだけ**を notify するようになり、notify されないフレームが
  日常的に起きるようになって顕在化した。加えて子の render は掃き出しの**後**に走るので、
  `hide_all`（D&D / パレット / close 確認との重なり回避）も子に上書きされて効いていなかった
- 直し方: フレーム同期をルート render（`sync_webview_frames`）へ移し、**どのウィンドウから
  呼ばれても同じ答えになる材料だけ**（全ウィンドウ共有の `pane_text_areas`。#339）から
  毎フレーム決め切る。印は撤去。A/B は `TAKO_838_NO_ROOT_WEBVIEW_SYNC=1`
- 実測（隔離 GUI・同一バイナリ A/B・20 秒 ×2 往復。root render の生存は分割比を 2 秒ごとに
  動かす能動プローブ `bounds_delta=10` が両側同値で担保）: 可視 ⇔ 不可視の切替が
  **178 / 174 回（8.9 / 8.7 回/秒）→ 0 回**、終了時の状態は `visible=False`（消えたまま）
  → `visible=True`。セルフテスト項目 71 に回帰検査を新設（旧経路では
  `visible=true → false`（切替 3 → 4）で FAILED になることを実測）
- 関連コミット: PR（Closes #838）
