# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-13・D&D 3 件完了直後）

- **ドラッグ&ドロップ 3 件を実装**（FR-2.16.10 / FR-3.11 / FR-1.10）:
  ① 統合 tmux ビューのセッション（attach 済み・管理外・kill漏れ）を D&D でタブ内へ
  取り込み = 分割ペインで `TMUX= tmux attach`（dispatch `TmuxOpen` + `tako tmux open` +
  MCP `tako_tmux_open` = **計 22 ツール**） ② ファイルツリーのファイルを D&D で
  ドロップ位置にプレビュー（`OpenFile` に `direction` 追加。中央 40% = 従来の再利用）
  ③ ペインタイトルバーの D&D で同タブ内移動（iTerm2 流。`Workspace::move_pane_to` +
  `MovePane` の `target`/`direction` 拡張）。3 件共通でドロップ先ハイライト +
  挿入プレビュー（象限 → 半面強調 + 結果ラベル）。実装メモは `requirements.md`
  FR-2.16.10 / FR-3.11 実装メモ節
- 直前の完了: ワークスペース第 1 弾（FR-3.1 改 / 3.2 / 3.3。コードプレビュー /
  Markdown トグル / マルチルートツリー）
- セルフテスト **120 項目**緑（68 = tmux open e2e / 68b = OpenFile direction /
  68c = MovePane target 追加）・cargo test 全緑・clippy / fmt 緑・`.app` 反映済み・
  **ユーザーの再起動 + manual-checks「ドラッグ&ドロップ 3 件」
  「ワークスペース機能第 1 弾」両節の実機確認待ち**
- 最終更新: 2026-06-13

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.6 git graph（git CLI 子プロセス。パネルの git ビュー =
      プレースホルダを差し替える）→ FR-3.5 軽い編集 / FR-3.10 画像プレビュー /
      FR-3.9 diff ビューア + FR-2.7 show_file/show_diff
- [ ] **FR-2.19 localhost ポートパネル**(パネル UI 刷新済みで土台あり。要件登録済み)
- [ ] **FR-2.18 未表示の子の自動サーフェス**（フェーズ未定。要件登録済み）
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須。FR-2.14.6 含む）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）
- [ ] 常用確認: manual-checks.md「ドラッグ&ドロップ 3 件」「ワークスペース機能第 1 弾」
      「実機バグ 3 件一括修正」各節
- [ ] 描画のグリッド不一致（全角 advance ≠ セル幅 ×2）の要否判断
- [ ] プレビューの既知の制約: 長いコード行の横スクロール未対応 / 画像はエラー表示
      （FR-3.10 で対応）/ ファイル変更の自動リロードなし（開き直しで更新）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜。Actions 無料枠
  90% 到達のためユーザーが停止。workflow ファイルは有効なまま）。push 後に CI 実行が
  作成されないのは正常 → CI 待ちポーリングはしない。品質保証はローカルの
  セルフテスト + `cargo test --workspace` + fmt + clippy 全緑で足りる扱い
- **プレビューペインは terminals に居ない**: `terminals.get(pane)` が None でも正常系。
  ペイン横断の処理（集約・kill・close）は previews も見ること（close 系 3 経路 +
  detach_session で previews を掃除済み）。`render_pane` の返り値は `AnyElement` 化済み
- **dispatch OpenFile はセッション起動を伴わない** → セルフテストで直接 dispatch して
  よい（Split の pending_attach の罠の対象外。項目 56 コメント参照）。
  **TmuxOpen はセッション起動を伴う** → 直接 dispatch 後に pending_attach 処理が必要
- **zsh の equals 展開の罠**: 明示コマンドの引数が `=` で始まると `$SHELL -l -c` 経由で
  化ける（`tmux attach -t =name` で実測）→ `quote_word` が先頭 `=` を必ずクォートする
- **GPUI の D&D**: bubble は登録の逆順 → gpui 内部のドラッグ準備リスナー（後登録）は
  ユーザー listener の stop_propagation より先に走る = タイトルバーの focus 用
  on_mouse_down と on_drag は共存できる。drop 成立時は on_drop が stop_propagation
  するためルート on_mouse_up は走らない（非成立時のみそこでドラッグ状態をクリア）
- **GPUI（taffy）の flex 子は overflow: visible だと自動最小サイズ = min-content**:
  スクロールしない固定バーを flex 列に置くときは「可変領域に `min_h(0)` +
  固定バーに `flex_none()`」をセットで付ける（ステータスバー消失バグの教訓）
- **統合 tmux ビューのデータ層**: `tmux_view_groups()`（タブ枠 + FR-2.16.9 の attach
  紐付け）+ `tmux_unlisted_sessions()`（管理外 / orphan）。プレビューペインの行ラベルは
  「📄 ファイル名」
- **CSI u の送出範囲は `CsiUMode`**（main.rs）: Full = アプリが kitty 要求済み /
  ModifiedOnly = バックエンドペイン強制（Esc は素の `\e`）/ Off = レガシー
- **スクロール関連の罠**: ペインターゲットは `=セッション名:`（末尾コロン必須）。
  extended-keys は always 必須。conf はサーバー起動時のみ → 稼働サーバーへは `sync_conf`
- **ネスト tmux の推奨設定の正は `tmux_backend::NESTED_TMUX_SNIPPET`**
- **接続情報**: `instances/control-<pid>.json` + current。CLI は生存候補へ自動フォールバック
- セルフテストは **114 項目**。IME 項目は稀にフレーク（再実行で緑）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- FR-3.6 git graph 着手時: `architecture.md`「コンセプト②の実現」（git CLI 方式）+
  `requirements.md` FR-3.6 / FR-2.16（パネルの git ビュー差し替え）
- FR-3.5 / FR-3.9 / FR-3.10 着手時: `requirements.md` FR-3 実装メモ +
  `crates/tako-app/src/preview.rs`（プレビュー基盤に乗せる）
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
