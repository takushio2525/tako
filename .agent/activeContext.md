# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **Phase 4 完了**（2026-06-12）: FR-2.12（AI 自動リネーム）+
  FR-2.4.2〜4（listen 検知 + 提案チップ + OFF 設定）+ FR-2.10（集約センター =
  右端固定タブ「agents」、注目度順、dispatch Focus でジャンプ）。MCP は計 18 ツール。
  FR-2.14（MCP ゼロコンフィグオンボーディング）は要件登録済み（実装は Phase 7 前）。
  次は **Phase 5（ワークスペース機能）** か FR-2.14 前倒しをユーザーと相談
- ステータス: セルフテスト 84 項目緑。集約センター分のコミット・push と CI / .app
  更新を確認中
- 最終更新: 2026-06-12

## 直近の観点・指摘

- **自動リネームの構造**（`tako-app/src/autorename.rs`）: 判断はプロンプト 1 本に閉じる。
  素材指紋に**画面末尾を含めない**（実行中に毎 tick 変わり静穏にならない）。
  claude 解決はログインシェル経由 `command -v claude`（GUI の PATH 最小問題対策、
  `TAKO_CLAUDE_BIN` で差し替え可）。`TAKO_SELF_TEST` 中はループ無効 + claude 不使用 +
  設定を永続化しない
- **listen ポート検知**（`tako-core::ports`）: libc に無い `socket_fdinfo` は SDK ヘッダ
  転記 + **自プロセス listen のユニットテストで ABI 検証**。バッファは align 1 のため
  `read_unaligned` 必須。「ペイン配下」= 制御端末（`proc_bsdinfo.e_tdev`）と PTY スレーブ
  rdev の一致（プロセスツリー走査より単純でジョブ全体を拾う）。Linux / Windows は空で劣化
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

- Phase 5 着手時: `roadmap.md`「Phase 5」+ `architecture.md`「コンセプト②の実現」
  「Web ビューペイン」節。**技術選定（ハイライタ等）は候補 2〜4 個 + 推奨 1 つ +
  各トレードオフ 1 行の形でユーザーへ提示して止まる**（roadmap Phase 5 末尾に明記）
- Phase 5.5（tmux バックエンド永続化。全 PTY の tmux session 化・完全復元、
  ユーザー承認済み方式）を Phase 5 の次に登録済み → `roadmap.md`「Phase 5.5」
- FR-2.14 着手時: `requirements.md` FR-2.14 の実装メモ

## 未解決・次の一手

- [ ] **次フェーズの相談**: Phase 5（ワークスペース機能）に進むか、FR-2.14
      （MCP ゼロコンフィグオンボーディング）を前倒すか
- [ ] /Applications の .app 更新 + ユーザー再起動
- [ ] 常用確認: 自動リネーム（claude 実呼び出し）+ 提案チップ（実 dev サーバー）+
      集約センター（見た目）→ manual-checks.md
- [ ] 描画のグリッド不一致（全角 advance ≠ 2 セル）の根本対応の要否を常用で判断

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
