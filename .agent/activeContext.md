# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-12 夜）

- **スクロール・キー実機バグ一括（4 点 + 品質 2 点）完了**: ① ホイール無反応
  （ネスト tmux の mouse off + トラックパッド端数切り捨て）② 右上の時刻表示
  （tmux 3.6 copy-mode インジケータ）③ スクロールバー（iTerm2 流フェード表示で復活）
  ④ Shift+Enter（ネスト tmux の extended-keys。**always 必須**・on では不可）
  ⑤ スクロールのヌルヌル化（コアレッシング）⑥ カーソル居残り（copy-mode 中の抑止）
- **スクロール制御は方式転換済み**: SGR を tmux 既定バインドに任せず、
  `tako-core::scroll` が実体（バックエンド / ネスト先ユーザー tmux。tty 突き合わせで
  解決）の copy-mode を正確な行数で駆動。キー入力前に cancel（iTerm2 流）。
  UI / CLI / MCP は同一経路（dispatch）。詳細は `architecture.md`「スクロール制御」節
- **ユーザー環境設定済み**: `~/.tmux.conf` を新設し稼働中の既定サーバーへ適用済み
  （mouse on / extended-keys always / extkeys features / インジケータ非表示）。
  内容の正は `tmux_backend::NESTED_TMUX_SNIPPET`。
  **注意: 設定適用前から起動中の claude には Shift+Enter が効かない（再起動で有効）**
- ステータス: セルフテスト 105 項目緑・core e2e（scroll 4 本 / ネストチェーン 2 本 /
  インジケータ / sync_conf 含む）緑・/Applications へ .app 反映済み・CI は最終 push 分を確認中
- 最終更新: 2026-06-12

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 再開**: FR-3.2 コードプレビュー + `tako_open_file`（下記「再開手順」）→
      FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス。
      **右サイドバー情報パネルの内部タブとして追加** = FR-2.16.2）
- [ ] **パネル UI 系の変更**（ユーザーが別途タスクとして投げる予定と明言。待ち）
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（2026-06-12 要件化。
      検出 → 診断表示 → 案内 + ボタン一発 / MCP 適用。勝手に書き換えない。Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）
- [ ] **配布・自動アップデート要件の roadmap 登録**（Phase 7 に項目化すること自体が未着手）
- [ ] **「表示されていない子の自動サーフェス」FR 登録**（FR 採番から）
- [ ] 常用確認の残り: manual-checks.md「スクロール・キー実機バグ一括」節ほか
- [ ] 描画のグリッド不一致（全角 advance ≠ セル幅 ×2）の要否判断

## Phase 5 再開手順（FR-3.2 から）

1. プレビューペイン種別の導入（app 側 `previews: HashMap<PaneId, …>` が現構造に素直。
   terminals と同居）
2. syntect 依存追加（**純 Rust 構成**: default-features = false + `regex-fancy` /
   `default-syntaxes` / `default-themes`）。**`Highlighter` trait で抽象化**（ユーザー指示）
3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
   （開発不変条件。ツール 21 個目）
4. `main.rs` の `open_file_row()` がプレースホルダで待っている

## 直近の観点・指摘（実装時に踏みやすい点）

- **スクロール関連の罠**: tmux のペインターゲットは `=セッション名:`（末尾コロン必須）。
  tmux はペインからの kitty 要求（`\e[>1u`）を認識しない → extended-keys は always。
  terminal-features の extkeys 明示が無いとネスト tmux は CSI u 入力を**捨てる**。
  conf はサーバー起動時のみ読まれる → 稼働サーバーへは `sync_conf`（起動時に呼ぶ）
- **tmux バックエンドの要点**: spawn は `tmux_backend::wrap_options`、レイアウトは
  `tako-control::layout`、close 整合は requirements.md FR-5 節。罠は architecture.md
  「Phase 5.5」節 + 「スクロール制御」節
- **バックエンドペインは disambiguate 常時 ON**（handle_key）。マウス / CJK / CSI u /
  ネストチェーンの保証は core e2e（tmux_backend / scroll）が回帰防止
- **接続情報**: `instances/control-<pid>.json` + current。CLI は生存候補へ自動フォールバック
- **設定**: `<data_dir>/settings.json`（auto_rename / port_detect / tmux_persist）
- セルフテストは **105 項目**。IME 項目は稀にフレーク（再実行で緑）。tmux 項目は
  隔離ソケット + kill-server、接続情報は隔離ディレクトリ
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 5 再開時: 上の「再開手順」+ `architecture.md`「コンセプト②の実現」
- スクロール / ネスト tmux に触るとき: `architecture.md`「スクロール制御」+
  `requirements.md` FR-2.17
- たまり場・パネル拡張時: `requirements.md` FR-2.15 / FR-2.16 + FR-5 の close 整合節
- オンボーディング着手時: `requirements.md` FR-2.14 + `concept.md` ビジョン節

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
