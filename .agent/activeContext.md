# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **FR-2.12（タブ・ペイン名の AI 自動リネーム）完成**（2026-06-12、
  方式 1 = tako 常駐をユーザー承認済み）。検知ループ（2 秒ポーリング + 静穏 4 秒 +
  クールダウン 30 秒）→ `claude -p`（haiku 固定）→ `set_title_auto` 反映。手動優先は
  `TitleSource`、OFF は settings.json + `tako autorename` / MCP（計 17 ツール）。
  次は **Phase 4 後半**（listen ポート検知 FR-2.4.2 → 提案チップ → 集約センター FR-2.10）
- ステータス: セルフテスト 77 項目緑・push 済み。**/Applications の .app は要更新**
  （`scripts/build-app.sh --install`）。claude 実呼び出し経路は常用で確認
  （manual-checks.md「AI 自動リネーム」）
- 最終更新: 2026-06-12

## 直近の観点・指摘

- **自動リネームの構造**（`tako-app/src/autorename.rs`）: 判断はプロンプト 1 本に閉じる。
  素材指紋に**画面末尾を含めない**（実行中に毎 tick 変わり静穏にならない）。
  claude 解決はログインシェル経由 `command -v claude`（GUI の PATH 最小問題対策、
  `TAKO_CLAUDE_BIN` で差し替え可）。`TAKO_SELF_TEST` 中はループ無効 + claude 不使用 +
  設定を永続化しない
- **タブ表示名の優先順位**: title_source = default のタブのみ「フォーカスペインの OSC
  タイトル」フォールバック。auto / manual はタブ名を表示（OSC に上書きされない）
- **ホイールの出し分け**（`wheel_action`）: mouse reporting → SGR/X10 転送、
  alt screen + alternate scroll → 矢印変換、通常画面 → 自前スクロールバック
- **kitty keyboard protocol**: `Config.kitty_keyboard = true` が必須。disambiguate 中は
  Esc / 修飾付き Enter・Tab・Backspace を CSI u で送出（`keystroke_to_bytes`）
- **描画とグリッドのずれ（重要な残課題）**: 全角 advance ≠ セル幅 ×2。マウス座標は
  shaping ベース変換で吸収済みだが描画自体のグリッド不一致は未解決（根本対応は
  セル単位グリッド描画。常用で気になったら着手）
- **IME 候補位置**: ライブ変換の文書全体オフセットは `clamp_ime_range_start` で
  marked text 内へ解釈
- **接続情報の発見**（FR-2.2.9）: `<data_dir>/control.json`（0600）。env → ファイルの順。
  ユーザー設定は `<data_dir>/settings.json`（`tako-control::settings`、秘密を含めない）
- **tmux 対応付け**: TIOCPTYGNAME の tty 名 × tmux client_tty 突き合わせ（macOS のみ）
- セルフテストは **77 項目**。IME 項目はタイミングで稀にフレーク（再実行で緑）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 4 後半着手時: `architecture.md`「Layer 3」節 + `requirements.md` FR-2.4.2〜2.4.4 /
  FR-2.10（listen ポート検知は macOS: libproc）

## 未解決・次の一手

- [ ] Phase 4 後半: listen ポート検知（FR-2.4.2）→ 提案チップ（FR-2.4.3〜4）→
      集約センター（FR-2.10）
- [ ] /Applications の .app 更新（`scripts/build-app.sh --install`）+ ユーザー再起動
- [ ] 常用確認: 自動リネームの claude 実呼び出し経路（manual-checks.md「AI 自動リネーム」）
- [ ] 描画のグリッド不一致（全角 advance ≠ 2 セル）の根本対応の要否を常用で判断

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
