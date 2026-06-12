# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **Phase 5.5 完了 + 実機リグレッション一括修正（2026-06-12）**。
  ① tmux_bin のログインシェル解決（.app の最小 PATH で tmuxview 空 / バックエンド沈黙劣化
  / 明示コマンド split 失敗の根本原因）② マウス / CSI u の tmux 越し生配送保証
  （バックエンドペインは disambiguate 常時 ON + conf `extended-keys-format csi-u`）
  ③ IME 候補位置を shaping 化（全角行のずれ根治）④ 固定タブ 0 個化 = 右サイドバー
  情報パネル（tmux / agents 内部タブ、ドラッグ幅調整、`tako panel` / MCP `tako_panel` =
  計 20 ツール）⑤ ペインタイトルバー（× + 状態ドット + リネーム名）
- ステータス: セルフテスト 98 項目緑・core e2e 4 本緑。.app 反映は本ターン末尾参照
- 最終更新: 2026-06-12

## 次の一手 = Phase 5 再開（FR-3.2 コードプレビュー + `tako_open_file`）

1. プレビューペイン種別の導入（app 側 `previews: HashMap<PaneId, …>` が現構造に素直。
   terminals と同居）
2. syntect 依存追加（**純 Rust 構成**: default-features = false + `regex-fancy` /
   `default-syntaxes` / `default-themes`。oniguruma の C 依存回避 = Windows CI 配慮）。
   **`Highlighter` trait で抽象化**し将来 tree-sitter へ差し替え可能に（ユーザー指示）
3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
   （開発不変条件。ツール 21 個目）
4. `main.rs` の `open_file_row()` が**プレースホルダで待っている**
   （ファイルツリーのファイル行クリック → ここからプレビューを開く）
- その後: FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス、
  レーン割当は純関数 + ユニットテスト）
- サイドバーの実装パターン: `content_origin.x` をサイドバー幅分ずらすだけで
  ペイン矩形・境界ハンドル・IME 位置がすべて連動する（render 冒頭参照）

## 直近の観点・指摘

- **tmux バックエンド（Phase 5.5）の要点**: spawn は `tmux_backend::wrap_options`、
  レイアウトは `tako-control::layout`（layout.json、同一 ID 復元）、close 整合は
  requirements.md FR-5 節。スパイクで踏んだ罠（既定シェルにコマンドを渡さない /
  `$'\e'` 置換 / `display-message -p`）は architecture.md「Phase 5.5」節に記録
- **たまり場（FR-2.15）**: 要件登録のみ（2026-06-12）。× = kill → 退避への変更は
  UI の見せ方をユーザーと相談してから。バックエンドの orphan セッション構造が前提
- **自動リネーム**（`autorename.rs`）: 判断はプロンプト 1 本。`TAKO_SELF_TEST` 中は
  ループ無効 + claude 不使用 + 設定を永続化しない
- **listen 検知**（`tako-core::ports`）: バックエンドペインは tty を tmux 側ペイン tty に
  差し替えて突き合わせ維持（`set_tty_name`）。チップ承諾は `open_preview`（Phase 5 で差し替え）
- **設定**: `<data_dir>/settings.json`（auto_rename / port_detect / tmux_persist）。
  トグルは dispatch 経由（`tako autorename` / `tako portdetect` / `tako persist` + MCP）
- **描画とグリッドのずれ（残課題）**: 全角 advance ≠ セル幅 ×2。座標変換は shaping で
  吸収済み、描画自体は未対応
- セルフテストは **98 項目**。IME 項目はタイミングで稀にフレーク（再実行で緑）。
  tmux 項目は隔離ソケット（`TAKO_TMUX_SOCKET=tako-st-<pid>`）+ 終了時 kill-server。
  ファイルツリー root 追従はオクルージョン対策で render 非依存化済み（sync_filetree_root）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 5 再開時: 上の「次の一手」+ `architecture.md`「コンセプト②の実現」
- たまり場に触るとき: `requirements.md` FR-2.15 + FR-5 の close 整合節（着手前にユーザーと UI 相談）

## 未解決・次の一手

- [ ] Phase 5 再開: FR-3.2 コードプレビュー + tako_open_file（上記）
- [ ] 常用確認: manual-checks.md「tmux バックエンド永続化」+「実機リグレッション修正一括」節
      （再起動復元 / claude のホイール・Shift+Enter / IME 位置 / パネル / タイトルバー）
- [ ] たまり場（FR-2.15）: UI の見せ方をユーザーと相談（実装は当分先で OK）
- [ ] 描画のグリッド不一致の根本対応の要否を常用で判断

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
