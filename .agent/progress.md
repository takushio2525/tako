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
