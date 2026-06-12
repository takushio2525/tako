# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-12 夜・パネル UI 刷新 完了直後）

- **パネル UI 刷新（FR-2.16.4〜2.16.8）完了**（コミット c91f7b3・CI 確認中に引き継ぎ）:
  - 下部ステータスバー新設（左 = ◫ ファイル、右 = ⌗ tmux / ⎇ git）。「◧ panel」廃止
  - パネル内部タブ 1 本化: agents → 統合「tmux」ビュー（タブ名ラベル枠 + 全ペイン入れ子 +
    ゴミ箱 → 確認 → dispatch Close）。旧 tmuxview 削除。git ビューはプレースホルダ
  - FR-2.16.8（実装中のユーザー追加要件）: タブ未表示の tmux を「管理外 /
    kill漏れ?」ラベルで区別表示 + 確認つき TmuxKill
  - ファイルツリーの CLI / MCP 経路新設: `Request::Panel` に `filetree` 追加
    （`tako panel --filetree on/off` / MCP `tako_panel`。ツール数は 20 のまま）。
    パネル view の wire 値は `tmux | git`（**agents は廃止**）
- セルフテスト **107 項目**緑・workspace テスト緑・clippy / fmt 緑・
  `.app` を /Applications へ反映済み・**ユーザーの再起動待ち**（前回のスクロール根治 +
  署名安定化 + 今回の UI 刷新がまとめて次回起動から有効）
- 最終更新: 2026-06-12

## 既知バグ（次の worker が修正）

- [ ] **Escape で「27u」が入力欄に挿入されることがある**（2026-06-12 報告）。
  extended-keys（CSI u）対応の副作用。tako の kitty / CSI-u 対応（`handle_key`）と
  ネスト tmux の extended-keys 設定（`NESTED_TMUX_SNIPPET`）の整合を調査して直すこと。
  関連: `architecture.md`「スクロール制御」節 + FR-2.17 実装メモ、core e2e の CSI u 往復

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 再開**: FR-3.2 コードプレビュー + `tako_open_file`（下記「再開手順」）→
      FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス。
      パネルの git ビュー = プレースホルダを差し替える）
- [ ] **FR-2.19 localhost ポートパネル**（パネル UI 刷新済みで土台あり。要件登録済み）
- [ ] **FR-2.18 未表示の子の自動サーフェス**（フェーズ未定。要件登録済み）
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須。FR-2.14.6 含む）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）
- [ ] 常用確認: manual-checks.md「パネル UI 刷新」節 +「スクロール・キー実機バグ一括」節
- [ ] 描画のグリッド不一致（全角 advance ≠ セル幅 ×2）の要否判断

## Phase 5 再開手順（FR-3.2 から）

1. プレビューペイン種別の導入（app 側 `previews: HashMap<PaneId, …>` が現構造に素直）
2. syntect 依存追加（純 Rust 構成: default-features = false + `regex-fancy` /
   `default-syntaxes` / `default-themes`）。`Highlighter` trait で抽象化（ユーザー指示）
3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
   （開発不変条件。ツール 21 個目）
4. `main.rs` の `open_file_row()` がプレースホルダで待っている

## 直近の観点・指摘（実装時に踏みやすい点）

- **セルフテストで dispatch を直接呼ぶときの罠**（今回踏んだ）: `Request::Split` を
  テストクロージャ内から直接 dispatch すると `pending_attach` が処理されず、後続の
  CLI dispatch が「ツリーに居ないペイン」を起動して以降の項目が壊れる。
  直接 dispatch した後は `std::mem::take(&mut app.pending_attach)` → `spawn_session` を
  自前で回すこと（項目 56 が実例）
- **統合 tmux ビューのデータ層**: `tmux_view_groups()`（タブ枠）+
  `tmux_unlisted_sessions()`（管理外 / orphan の分類）。表示と分離済み（FR-2.13.5）
- **スクロール関連の罠**: ペインターゲットは `=セッション名:`（末尾コロン必須）。
  extended-keys は always 必須。conf はサーバー起動時のみ → 稼働サーバーへは `sync_conf`
- **ネスト tmux の推奨設定の正は `tmux_backend::NESTED_TMUX_SNIPPET`**
- **バックエンドペインは disambiguate 常時 ON**（handle_key）。core e2e が回帰防止
- **接続情報**: `instances/control-<pid>.json` + current。CLI は生存候補へ自動フォールバック
- セルフテストは **107 項目**。IME 項目は稀にフレーク（再実行で緑）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 5 再開時: 上の「再開手順」+ `architecture.md`「コンセプト②の実現」
- FR-2.19 ポートパネル着手時: `requirements.md` FR-2.19 + FR-2.16（パネルのビュー追加）
- スクロール / ネスト tmux に触るとき: `architecture.md`「スクロール制御」+
  `requirements.md` FR-2.17
- 配布・オンボーディング着手時: `roadmap.md` Phase 7 + `requirements.md` FR-2.14 +
  `concept.md` ビジョン節

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
