# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **常用初日バグ 3 件修正 + ペイン境界ドラッグリサイズ実装完了**。
  Phase 3.5（IME・.app）は既に完了済み。次はユーザーの日常常用継続と
  Phase 4（パッシブ検知 + role/状態表示 UI）
- ステータス: push 後の CI（macOS / Windows）緑確認待ち
- 最終更新: 2026-06-11

## 直近の観点・指摘

- **IME の要点**: `EntityInputHandler` で未確定文字列を擬似ドキュメントとして公開
  （ターミナルに文書は無い）。確定 = PTY へ write、変換中 = カーソル位置のオーバーレイ描画。
  `handle_input` は paint 限定 API → canvas の paint フックから登録。
  **`handle_key` の `stop_propagation()` が二重入力防止の要**（外すと macOS が未処理キーを
  IME へ回送し insertText で二重入力する）。StyledText のハイライトは**非重複・昇順**必須
- **実 IME の見た目は未手動確認**: セルフテストは状態遷移のみ。`.agent/manual-checks.md` の
  IME チェックリストをユーザーの初回常用時に通すこと
- **.app**: `scripts/build-app.sh [--verify|--install]`。tako CLI を MacOS/ に同梱
  （`claude mcp add --scope user tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve`
  の登録パスを安定させるため）。ad-hoc 署名のみ（配布署名 / notarization は Phase 7）。
  bash は `$VAR（` の全角文字を変数名に含めて解釈するので `${VAR}` で括ること
- **常用初日バグの要点**: ①シェルの `TERM` を未設定で親（.app は Finder 由来で不定）から
  継承していた → tmux が「missing or unsuitable terminal」で落ちる。`TerminalSession::spawn`
  で `TERM=xterm-256color`/`COLORTERM=truecolor` を既定注入（`options.env` 優先）。
  ②初期 cwd が `/`（.app の親 cwd）→ `$HOME` 既定に（元ペイン継承は OSC 7 で Phase 4）。
  ③Backspace のバイト変換（`keystroke_to_bytes`）は `\x7f` で正しく、症状は①の二次効果
  （不正 TERM で zle が terminfo を読めず DEL が空白描画）。総点検の unit test を追加
- **境界ドラッグリサイズ**: 換算ロジックは tako-core（`PaneTree::borders`/`set_split_ratio`/
  `ratio_for_position`、pre-order index で分割を一意特定）。UI は薄く、各境界に数 px の
  透明ハンドル div を重ね `cursor` と `on_mouse_down`→`start_border_drag`、移動は
  グローバル `on_mouse_move` で `set_split_ratio`、PTY は次 render の `render_pane` で追従
- セルフテストは **44 項目**（1b〜1e: TERM/cwd/tmux/Backspace、5b: 境界ドラッグ、
  37〜39: IME）。セルフテスト future はメインスレッドで動くため
  dispatch 往復を伴うブロッキングは background executor へ（デッドロックの教訓）
- gpui ハマりどころ（font-kit feature 必須等）は `poc/README.md` / `architecture.md` 参照。
  gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0。
  zed の editor/terminal クレートは GPL 系なので読まない）

## 現フェーズで Read すべき設計書

- 常用開始時: `.agent/manual-checks.md`（IME + .app の手動チェックリスト）
- Phase 4 着手時: `.agent/architecture.md`「Layer 3」節 + `requirements.md` FR-2.4 / FR-2.10 /
  FR-2.1.3〜2.1.4（role/状態表示 UI は Phase 3 残をここで回収）

## 未解決・次の一手

- [ ] ユーザーの日常常用開始（manual-checks.md の IME / .app チェックを通す。
      フィードバックは FR へ反映）
- [ ] Phase 3 残: role ラベル / 状態表示 UI（FR-2.1.3〜2.1.4。Phase 4 集約センターと併せて）
- [ ] Phase 4: パッシブ検知（OSC 7/133・listen ポート・提案チップ・集約センター）
- [x] Phase 1 残骨格: ドラッグでのペイン境界リサイズ（実装済み。見た目の手触りは常用で確認）
- [ ] Phase 5 送り: 画像プレビュー（FR-3.10）・Web ビュー（FR-3.8）・注釈（FR-2.6）・
      diff（FR-3.9）・提示系（FR-2.7）・フィードバック（FR-2.8）・cmd+K（FR-2.9）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
