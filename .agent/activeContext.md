# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **Phase 3 完全完了 + Phase 4 前半完了**（role/状態表示 UI、OSC 7/133
  シェル統合、state/cwd の list・MCP 公開、split cwd 継承）。
  常用クラッシュ 2 件（fd リーク→SIGABRT、境界ドラッグ判定残留）も根治済み。
  次は Phase 4 後半（listen ポート検知・提案チップ・集約センター）
- ステータス: push 済み。CI（macOS / Windows）緑確認待ち。
  /Applications の .app は**状態 UI・シェル統合より前の版** → 次回作業時か常用前に
  `scripts/build-app.sh --verify --install` で更新すること
- 最終更新: 2026-06-11（深夜セッション）

## 直近の観点・指摘

- **常用クラッシュの根治（最重要の教訓）**: macOS で alacritty 既定シェル（None）は
  setuid root の `login` ラッパ経由 → close 時の SIGHUP が権限エラーで効かず
  `Pty::drop` の `wait()` が永久ブロック → **close ごとに fd・IO スレッド・login が
  リーク** → fd 枯渇で PTY 生成失敗 → `spawn_session` の expect が FFI 境界 panic →
  SIGABRT。対策: ①`$SHELL` をユーザー権限で直接 spawn（`default_shell`、`-l` 付き）
  ②spawn_session を Result 化し失敗時はペイン巻き戻し + エラー応答。
  詳細は `architecture.md`「PTY セッション破棄のハマりどころ」節
- **OSC 7/133 検知の構造**: vte は OSC 7/133 を捨てる → `TapPty`（EventedPty 委譲
  ラッパ、`osc_tap.rs`）で PTY 読みバイト列をタップ。`SessionEvent::Term|Osc` 統合
  channel → `TerminalSession` に cwd / `CommandState`（Unknown/Idle/Running/Failed、
  エラーは次コマンド開始まで保持）。list に `cwd`/`state`/`exit_code` 公開済み
- **シェル統合**: zsh=ZDOTDIR / bash=PROMPT_COMMAND / fish=XDG_DATA_DIRS の 3 点を
  **シェル判定なしで常時注入**（互いに無害）。`TAKO_NO_SHELL_INTEGRATION=1` で無効化。
  スクリプトは `crates/tako-core/shell-integration/` → 実行時に
  `~/Library/Application Support/tako/shell-integration/` へ書き出し
- **UI**: ペイン右上バッジ（title · role + 状態ドット）、タブバーに集約状態ドット
  （`CommandState::aggregate`、Failed=赤 > Running=アクセント）。見た目の手触りは
  manual-checks.md の項目で常用時に確認
- セルフテストは **55 項目**（40/40b: close 回帰 + fd リーク検査、41/41b: シェル統合
  e2e + split cwd 継承、5c: ドラッグ状態残留）。IME「確定文字列が PTY へ」は
  タイミング起因で稀にフレーク（再実行で緑。要観察）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- Phase 4 後半着手時: `.agent/architecture.md`「Layer 3」節（listen ポート検知 =
  libproc・ポーリング方式）+ `requirements.md` FR-2.4.2〜2.4.4 / FR-2.10
- 常用フィードバック対応時: `.agent/manual-checks.md`（シェル統合 bash/fish・状態ドット・
  role バッジ・境界ドラッグの目視項目）

## 未解決・次の一手

- [ ] Phase 4 後半: listen ポート検知（FR-2.4.2、macOS: libproc ポーリング）→
      提案チップ UI（FR-2.4.3〜2.4.4）→ 待ちエージェント集約センター（FR-2.10）
- [ ] ユーザーの常用継続（.app 更新後、manual-checks.md の新項目を通す。
      特に bash/fish 統合と p10k 等プロンプトカスタマイズとの共存）
- [ ] IME セルフテストのフレーク（項目 38）が再発したら wait 延長を検討
- [ ] Phase 5 送り: 画像プレビュー（FR-3.10）・Web ビュー（FR-3.8）・注釈（FR-2.6）・
      diff（FR-3.9）・提示系（FR-2.7）・フィードバック（FR-2.8）・cmd+K（FR-2.9）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
