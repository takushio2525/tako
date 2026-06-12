# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **Phase 5 を一時中断し、Phase 5.5（tmux バックエンド永続化）へ**
  （2026-06-12 ユーザー指示。Phase 5.5 は別 worker が担当）。
  中断時点の Phase 5 進捗: 技術選定確定（syntect / git CLI 子プロセス / pulldown-cmark =
  `architecture.md`「コンセプト②の実現」）+ **ファイルツリー（FR-3.1 / FR-3.7）完成**
  （cmd+B トグル・cwd 追従・2 秒ポーリング。`tako-app/src/filetree.rs`）。
  Phase 4 は完了済み（FR-2.12 / FR-2.4.2〜4 / FR-2.10、MCP 計 18 ツール）
- ステータス: セルフテスト 86 項目緑・コミット / push 済み・CI 確認待ち。
  /Applications の .app は集約センター時点（ファイルツリー分は未反映 = 要再ビルド）
- 最終更新: 2026-06-12

## Phase 5 の中断点（再開時にここから）

- **次の一手 = FR-3.2 コードプレビュー + `tako_open_file`**:
  1. プレビューペイン種別の導入（Pane に kind を足すか、app 側 `previews:
     HashMap<PaneId, …>` マップか — 後者が現構造に素直。terminals と同居）
  2. syntect 依存追加（**純 Rust 構成**: default-features = false +
     `regex-fancy` / `default-syntaxes` / `default-themes` 系 feature。oniguruma の
     C 依存を避ける = Windows CI 配慮）。**`Highlighter` trait で抽象化**し
     将来 tree-sitter へ差し替え可能に（ユーザー指示）
  3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
     （開発不変条件。ツール 19 個目）
  4. `main.rs` の `open_file_row()` が**プレースホルダで待っている**
     （ファイルツリーのファイル行クリック → ここからプレビューを開く）
- その後: FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス、
  レーン割当は純関数 + ユニットテスト）
- サイドバーの実装パターン: `content_origin.x` をサイドバー幅分ずらすだけで
  ペイン矩形・境界ハンドル・IME 位置がすべて連動する（render 冒頭参照）

## 直近の観点・指摘

- **自動リネーム**（`autorename.rs`）: 判断はプロンプト 1 本。素材指紋に画面末尾を
  含めない。claude 解決はログインシェル経由（`TAKO_CLAUDE_BIN` で差し替え可）。
  `TAKO_SELF_TEST` 中はループ無効 + claude 不使用 + 設定を永続化しない
- **listen 検知**（`tako-core::ports`）: socket_fdinfo は SDK 転記 + 自プロセス listen の
  ユニットテストで ABI 検証。バッファ読みは `read_unaligned` 必須。チップ承諾は
  `open_preview`（Phase 5 の Web ペインへの差し替え点）
- **設定**: `<data_dir>/settings.json`（auto_rename / port_detect）。トグルは dispatch
  経由（`tako autorename` / `tako portdetect` + MCP）
- **描画とグリッドのずれ（残課題）**: 全角 advance ≠ セル幅 ×2。座標変換は shaping で
  吸収済み、描画自体は未対応
- セルフテストは **86 項目**。IME 項目はタイミングで稀にフレーク（再実行で緑）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 5.5（tmux バックエンド永続化）着手時: `roadmap.md`「Phase 5.5」+
  `architecture.md`「Layer 1〜3」節 + `tako-core/src/terminal.rs`（spawn 経路）と
  `tako-core/src/tmux.rs`（既存 tmux 取得層）。tmux 不在環境では直接 spawn へ無害劣化
- Phase 5 再開時: 上の「Phase 5 の中断点」+ `architecture.md`「コンセプト②の実現」

## 未解決・次の一手

- [ ] **Phase 5.5: tmux バックエンド永続化**（別 worker。roadmap 参照）
- [ ] Phase 5 再開: FR-3.2 コードプレビュー + tako_open_file（上記中断点から）
- [ ] /Applications の .app 更新（`scripts/build-app.sh --install`）
- [ ] 常用確認: manual-checks.md（自動リネーム / チップ / 集約センター / ファイルツリー）
- [ ] 描画のグリッド不一致の根本対応の要否を常用で判断

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
