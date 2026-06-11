# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **常用フィードバック一括対応 + tmuxview（FR-2.13）完成**（2026-06-12）。
  tmuxview = 右端固定タブ + tmux 一覧（tty 突き合わせで tako タブ・ペイン対応付け）+
  確認つき kill + `tako tmux list/kill` + MCP 2 ツール（計 15 ツール）。
  次は FR-2.12（AI 自動リネーム。**実行体の設計分岐をユーザーへ報告済み・回答待ち**）→
  Phase 4 後半（listen ポート検知・提案チップ・集約センター）
- ステータス: push 済み・CI 緑確認待ち。**/Applications の .app は最新化済み**。
  ユーザーの実行中 tako は要再起動（修正反映 + control.json の自インスタンス復帰）
- 最終更新: 2026-06-12

## 直近の観点・指摘

- **ホイールの出し分け**（`wheel_action`）: mouse reporting → SGR/X10 転送、
  alt screen + alternate scroll → 矢印変換、通常画面 → 自前スクロールバック。
  alt screen へ転送した矢印が TUI 終了後に zle へ漏れるのは仕様（セルフテストでは
  ctrl-u で後始末）
- **kitty keyboard protocol**: `Config.kitty_keyboard = true` が必須（既定 false だと
  CSI > u の push が無視される）。disambiguate 中は Esc / 修飾付き Enter・Tab・Backspace
  を CSI u で送出（`keystroke_to_bytes`）。REPORT_ALL_KEYS 等は未対応（必要時に拡張）
- **描画とグリッドのずれ（重要な残課題）**: 行描画は StyledText のプロポーショナル実フォント
  幅で、全角 advance ≠ セル幅 ×2。マウス座標は shaping ベース変換（`cell_at` +
  `ScreenLine::cell_cols`）で吸収したが、**描画自体のグリッド不一致は未解決**
  （TUI の罫線・カーソル列ずれの見た目に影響しうる。根本対応はセル単位グリッド描画 =
  描画基盤の改修。常用で気になったら着手）
- **IME 候補位置**: macOS ライブ変換は文書全体基準のオフセットを渡してくる →
  `clamp_ime_range_start` で marked text（擬似ドキュメント）内へ解釈
- **接続情報の発見**（FR-2.2.9）: `<data_dir>/control.json`（0600）。CLI は env →
  ファイルの順で解決、接続不可・認証失敗のみフォールバック。MCP ブリッジは env あり時のみ
  （tako 外 0 ツール維持）
- **× ボタン / スクロールバー**: UI からも dispatch（CLI/MCP と同じコマンド層）を通す。
  scroll は `tako scroll --to/--delta` + `tako_scroll_pane`（MCP 13 ツール）+ list の
  scroll フィールド
- **tmux 対応付けの仕組み**: TerminalSession が spawn 時に TIOCPTYGNAME（macOS）で
  PTY スレーブ tty 名を保持 → dispatch TmuxList が tmux の client_tty と突き合わせ。
  Linux/Windows は未対応（None = 対応付けなしで劣化）
- セルフテストは **73 項目**。IME 項目 38 はタイミングで稀にフレーク（再実行で緑）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- FR-2.12（AI 自動リネーム）着手時: `requirements.md` FR-2.12 の実装メモ
  （リネーム実行体の分岐は実装前にユーザーへ報告）+ `architecture.md`「Layer 2」節
- Phase 4 後半着手時: `architecture.md`「Layer 3」節 + `requirements.md` FR-2.4.2〜2.4.4 / FR-2.10

## 未解決・次の一手

- [ ] FR-2.12: タブ・ペインの AI 自動リネーム（要件登録済み。タブ rename API の追加から）
- [ ] Phase 4 後半: listen ポート検知（FR-2.4.2）→ 提案チップ（FR-2.4.3〜4）→
      集約センター（FR-2.10）
- [ ] 描画のグリッド不一致（全角 advance ≠ 2 セル）の根本対応の要否を常用で判断
- [ ] ユーザー常用確認: manual-checks.md（× ボタン・スクロールバー・bash/fish 統合・
      状態ドット・Shift+Enter は Claude Code 実機で）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
