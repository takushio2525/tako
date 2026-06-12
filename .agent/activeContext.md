# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-13・実機報告バグ 3 件の一括修正直後）

- **実機バグ 3 件を根治**: ① 統合 tmux ビューの「管理外」誤判定 → attach クライアント
  tty ↔ tako ペイン対応（TmuxList の `clients[].tab/pane`）で**該当タブの枠内へ紐付け
  表示**（FR-2.16.9 として要件登録。window 一覧 + 確認つき kill 付き）
  ② kill 確認 UI の右見切れ → メッセージ行（折り返し）+ ボタン行の**縦積み**に共通化
  （`render_kill_confirm`）+ 他行の flex 制約総点検 ③ 下部ステータスバー消失 →
  根因は taffy の flex 子の自動最小サイズ（overflow: visible だと min-content）。
  中段に `min_h(0)` + 各バーに `flex_none()`（architecture.md「実機リグレッション」節）
- セルフテスト **109 項目**緑（61f = attach 紐付け e2e 2 項目追加）・workspace テスト緑・
  clippy / fmt 緑・`.app` を /Applications へ反映済み・**ユーザーの再起動待ち**
- 最終更新: 2026-06-13

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 再開**: FR-3.2 コードプレビュー + `tako_open_file`（下記「再開手順」）→
      FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス。
      パネルの git ビュー = プレースホルダを差し替える）
- [ ] **FR-2.19 localhost ポートパネル**（パネル UI 刷新済みで土台あり。要件登録済み）
- [ ] **FR-2.18 未表示の子の自動サーフェス**（フェーズ未定。要件登録済み）
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須。FR-2.14.6 含む）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）
- [ ] 常用確認: manual-checks.md「実機バグ 3 件一括修正」「パネル UI 刷新」
      「スクロール・キー実機バグ一括」「Esc『27u』挿入バグ修正」各節
- [ ] 描画のグリッド不一致（全角 advance ≠ セル幅 ×2）の要否判断

## Phase 5 再開手順（FR-3.2 から）

1. プレビューペイン種別の導入（app 側 `previews: HashMap<PaneId, …>` が現構造に素直）
2. syntect 依存追加（純 Rust 構成: default-features = false + `regex-fancy` /
   `default-syntaxes` / `default-themes`）。`Highlighter` trait で抽象化（ユーザー指示）
3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
   （開発不変条件。ツール 21 個目）
4. `main.rs` の `open_file_row()` がプレースホルダで待っている

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜。Actions 無料枠
  90% 到達のためユーザーが停止。workflow ファイルは有効なまま）。push 後に CI 実行が
  作成されないのは正常 → CI 待ちポーリングはしない。品質保証はローカルの
  セルフテスト + `cargo test --workspace` + fmt + clippy 全緑で足りる扱い
- **GPUI（taffy）の flex 子は overflow: visible だと自動最小サイズ = min-content**:
  スクロールしない固定バーを flex 列に置くときは「可変領域に `min_h(0)` +
  固定バーに `flex_none()`」をセットで付ける（ステータスバー消失バグの教訓）
- **統合 tmux ビューのデータ層**: `tmux_view_groups()`（タブ枠 + FR-2.16.9 の attach
  紐付け `tmux_sessions_attached_to()`）+ `tmux_unlisted_sessions()`（管理外 / orphan。
  attach 済みは除外）。表示と分離（FR-2.13.5）
- **CSI u の送出範囲は `CsiUMode`**（main.rs）: Full = アプリが kitty 要求済み
  （Esc も CSI 27u）/ ModifiedOnly = バックエンドペイン強制（Esc は素の `\e`）/
  Off = レガシー。tmux は CSI 27u を非要求ペインにも素通しする（e2e のカナリア
  eprintln が観測。これが変わったら ModifiedOnly の Esc 例外を再検討）
- **セルフテストで dispatch を直接呼ぶときの罠**: `Request::Split` を
  テストクロージャ内から直接 dispatch すると `pending_attach` が処理されず、後続の
  CLI dispatch が「ツリーに居ないペイン」を起動して以降の項目が壊れる。
  直接 dispatch した後は `std::mem::take(&mut app.pending_attach)` → `spawn_session` を
  自前で回すこと（項目 56 が実例）
- **スクロール関連の罠**: ペインターゲットは `=セッション名:`（末尾コロン必須）。
  extended-keys は always 必須。conf はサーバー起動時のみ → 稼働サーバーへは `sync_conf`
- **ネスト tmux の推奨設定の正は `tmux_backend::NESTED_TMUX_SNIPPET`**
- **接続情報**: `instances/control-<pid>.json` + current。CLI は生存候補へ自動フォールバック
- セルフテストは **109 項目**。IME 項目は稀にフレーク（再実行で緑）
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
