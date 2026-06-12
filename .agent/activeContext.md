# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（新 worker への引き継ぎ。2026-06-12）

- **Phase 5.5（tmux バックエンド永続化）は完了し、ユーザーの実機で復元検証 OK**:
  タブ・ペイン構成・実行中プロセス・画面内容・CLI/MCP 接続が再起動をまたいで完全復元
  されることをユーザーが 2 回目の再起動で確認済み。OS ウィンドウのジオメトリ
  （サイズ・位置・フルスクリーン/ズーム）も layout.json に保存・復元される（最終追補）
- 同日の実機リグレッション一括対応も完了: tmux_bin ログインシェル解決（.app の最小 PATH）/
  マウス・CSI u の tmux 越し生配送保証 / CJK ロケール（-u + LC_CTYPE=UTF-8 既定注入）/
  IME 候補位置の shaping 化 / 明示コマンドのログインシェル実行 / 接続情報の
  インスタンス分離 + 生存フォールバック（バグ 8）/ 固定タブ 0 個化（右サイドバー
  情報パネル FR-2.16 + `tako panel` / MCP `tako_panel` = 計 20 ツール）/ ペインタイトルバー
- ステータス: セルフテスト 101 項目緑・core e2e 6 本緑・CI（macOS / Windows）緑・
  /Applications の .app 反映済み
- 最終更新: 2026-06-12

## 未着手タスク（新 worker が拾う。優先順はユーザーと相談）

- [ ] **Phase 5 再開**: FR-3.2 コードプレビュー + `tako_open_file`（下記「再開手順」）→
      FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス。
      **右サイドバー情報パネルの内部タブとして追加**する設計が確定済み = FR-2.16.2）
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須。CLI の PATH 設置
      FR-2.14.5 含む。コンセプト: 非プログラマでもセットアップ全面代行 → concept.md）
- [ ] **FR-2.15 ターミナルのたまり場**（タブ外プール。× = kill → 退避へ変更。
      **UI の見せ方をユーザーと相談してから着手**。orphan バックエンドセッション構造は準備済み）
- [ ] **配布・自動アップデート要件の roadmap 登録**（GitHub Releases / notarization /
      Sparkle 等の自動更新。Phase 7 に項目化すること自体が未着手）
- [ ] **「表示されていない子の自動サーフェス」FR 登録**（エージェントの子プロセス・
      バックエンド orphan 等、画面に出ていない活動を tako が能動的に見せる要件。FR 採番から）
- [ ] 常用確認の残り: manual-checks.md「tmux バックエンド永続化」「実機リグレッション
      修正一括」「ウィンドウジオメトリの復元」節
- [ ] 描画のグリッド不一致（全角 advance ≠ セル幅 ×2。描画側未対応）の要否判断

## Phase 5 再開手順（FR-3.2 から）

1. プレビューペイン種別の導入（app 側 `previews: HashMap<PaneId, …>` が現構造に素直。
   terminals と同居）
2. syntect 依存追加（**純 Rust 構成**: default-features = false + `regex-fancy` /
   `default-syntaxes` / `default-themes`。oniguruma の C 依存回避 = Windows CI 配慮）。
   **`Highlighter` trait で抽象化**し将来 tree-sitter へ差し替え可能に（ユーザー指示）
3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
   （開発不変条件。ツール 21 個目）
4. `main.rs` の `open_file_row()` がプレースホルダで待っている
   （ファイルツリーのファイル行クリック → ここからプレビューを開く）

## 直近の観点・指摘（実装時に踏みやすい点）

- **tmux バックエンドの要点**: spawn は `tmux_backend::wrap_options`、レイアウトは
  `tako-control::layout`（layout.json、同一 ID 復元 + ウィンドウフレーム）、close 整合は
  requirements.md FR-5 節。罠（既定シェルにコマンドを渡さない / `$'\e'` 置換 /
  `display-message -p` / ロケール / PATH）は architecture.md「Phase 5.5」節に集約
- **バックエンドペインは disambiguate 常時 ON**（handle_key。tmux は拡張キーを外側へ
  伝えないため）。マウス / CJK / CSI u の保証は core e2e（tmux_backend::tests）が回帰防止
- **接続情報**: `instances/control-<pid>.json` + current。CLI は生存候補へ自動フォールバック
  （除外キーは socket+token ペア）。セルフテストは `TAKO_DISCOVERY_DIR` で完全隔離
- **設定**: `<data_dir>/settings.json`（auto_rename / port_detect / tmux_persist）。
  トグルは dispatch 経由（`tako autorename` / `tako portdetect` / `tako persist` + MCP）
- セルフテストは **101 項目**。IME 項目は稀にフレーク（再実行で緑）。tmux 項目は
  隔離ソケット（`TAKO_TMUX_SOCKET=tako-st-<pid>`）+ kill-server、接続情報は隔離ディレクトリ
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 5 再開時: 上の「再開手順」+ `architecture.md`「コンセプト②の実現」
- たまり場・パネル拡張時: `requirements.md` FR-2.15 / FR-2.16 + FR-5 の close 整合節
- オンボーディング着手時: `requirements.md` FR-2.14 + `concept.md` ビジョン節

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
