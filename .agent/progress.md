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
