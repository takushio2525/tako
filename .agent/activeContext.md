# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-12 夜・Esc「27u」バグ根治直後）

- **既知バグ「Esc で 27u が入力欄に挿入」を根治**: 根因は tmux 3.6 が受信した
  CSI 27u を内側ペインの kitty 要求の有無に関係なく素通しすること（on / always
  どちらでも。実測）× tako がバックエンドペインで Esc 単押しを常に CSI 27u で
  送っていたこと。`CsiUMode`（Off / ModifiedOnly / Full）を導入し、バックエンド
  強制時は Esc 単押しを素の `\e` に（修飾付きキーの CSI u = Shift+Enter の生命線は
  維持）。詳細: `architecture.md`「実機リグレッション」節の拡張キー項
- 別件: ロケールカナリア（tmux.rs）が同一バイナリ・同一マシンで数時間内に挙動反転
  （C ロケールの TAB サニタイズが再現しなくなった）→ hard assert を観測 eprintln へ降格。
  修正本体の保証（tmux_command で TAB 保持・パース成功）は引き続き assert
- セルフテスト緑・workspace テスト緑・clippy / fmt 緑・`.app` を /Applications へ
  反映済み・**ユーザーの再起動待ち**（スクロール根治 + 署名安定化 + パネル UI 刷新 +
  今回の Esc 修正がまとめて次回起動から有効）
- 最終更新: 2026-06-12

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 再開**: FR-3.2 コードプレビュー + `tako_open_file`（下記「再開手順」）→
      FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス。
      パネルの git ビュー = プレースホルダを差し替える）
- [ ] **FR-2.19 localhost ポートパネル**（パネル UI 刷新済みで土台あり。要件登録済み）
- [ ] **FR-2.18 未表示の子の自動サーフェス**（フェーズ未定。要件登録済み）
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須。FR-2.14.6 含む）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）
- [ ] 常用確認: manual-checks.md「パネル UI 刷新」「スクロール・キー実機バグ一括」
      「Esc『27u』挿入バグ修正」各節
- [ ] 描画のグリッド不一致（全角 advance ≠ セル幅 ×2）の要否判断

## Phase 5 再開手順（FR-3.2 から）

1. プレビューペイン種別の導入（app 側 `previews: HashMap<PaneId, …>` が現構造に素直）
2. syntect 依存追加（純 Rust 構成: default-features = false + `regex-fancy` /
   `default-syntaxes` / `default-themes`）。`Highlighter` trait で抽象化（ユーザー指示）
3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
   （開発不変条件。ツール 21 個目）
4. `main.rs` の `open_file_row()` がプレースホルダで待っている

## 直近の観点・指摘（実装時に踏みやすい点）

- **CSI u の送出範囲は `CsiUMode`**（main.rs）: Full = アプリが kitty 要求済み
  （Esc も CSI 27u）/ ModifiedOnly = バックエンドペイン強制（Esc は素の `\e`）/
  Off = レガシー。tmux は CSI 27u を非要求ペインにも素通しする（e2e のカナリア
  eprintln が観測。これが変わったら ModifiedOnly の Esc 例外を再検討）
- **セルフテストで dispatch を直接呼ぶときの罠**: `Request::Split` を
  テストクロージャ内から直接 dispatch すると `pending_attach` が処理されず、後続の
  CLI dispatch が「ツリーに居ないペイン」を起動して以降の項目が壊れる。
  直接 dispatch した後は `std::mem::take(&mut app.pending_attach)` → `spawn_session` を
  自前で回すこと（項目 56 が実例）
- **統合 tmux ビューのデータ層**: `tmux_view_groups()`（タブ枠）+
  `tmux_unlisted_sessions()`（管理外 / orphan の分類）。表示と分離済み（FR-2.13.5）
- **スクロール関連の罠**: ペインターゲットは `=セッション名:`（末尾コロン必須）。
  extended-keys は always 必須。conf はサーバー起動時のみ → 稼働サーバーへは `sync_conf`
- **ネスト tmux の推奨設定の正は `tmux_backend::NESTED_TMUX_SNIPPET`**
- **接続情報**: `instances/control-<pid>.json` + current。CLI は生存候補へ自動フォールバック
- セルフテストは **107 項目**。IME 項目は稀にフレーク（再実行で緑）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 5 再開時: 上の「再開手順」+ `architecture.md`「コンセプト②の実現」
- FR-2.19 ポートパネル着手時: `requirements.md` FR-2.19 + FR-2.16（パネルのビュー追加）
- スクロール / ネスト tmux / 拡張キーに触るとき: `architecture.md`「スクロール制御」+
  「実機リグレッション」節 + `requirements.md` FR-2.17
- 配布・オンボーディング着手時: `roadmap.md` Phase 7 + `requirements.md` FR-2.14 +
  `concept.md` ビジョン節

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
