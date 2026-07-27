# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-27 夜・検知系 + UX + リリース基盤の 12 件を完遂 → v0.6.0 準備）

本日 merge 済み（すべて macOS CI 緑・install 済み・Issue CLOSED）:
- 検知系根治: #571（watch WORKER_IDLE）/ #572（busy 中入力のキュー化誤検知）/ #577（PERMISSION 検知）/
  #566（close ガード）/ #567（stale pane fallback）/ #599（セルフテスト 87）
- CI: #574（PWA ビルド工程 + Windows 非ブロッキング。**以後の合格条件 = macOS 全ジョブ緑**）
- UX: #549（初回バナー + パレット導線）/ #552（自動リネーム品質 4 点）/ #589（ツリー線）/
  #590（リモートインジケータ常時表示 + GUI 起動）/ #600（入力予測 zsh-autosuggestions 既定 ON）/
  #601（tako 内シェルへの PATH 自動注入）
- リリース基盤: #594 + #595（アセット命名規則の一元化 / 更新チェッカの自 OS フィルタ /
  release.sh の `--notes-only` `--update-notes`）

main 先端 = `c2c9350`。/Applications = 0.5.13（今夜の全修正入り）。

## 次の一手

1. **GUI 再起動**（master が実施。必ず `env -u CLAUDE_CONFIG_DIR open -a /Applications/tako.app`）
2. ユーザー目視: 初回バナー(#549)/リモートインジケータ(#590)/入力予測(#600)/ツリー線(#589)/
   #562 マージ導線 / #496 カード / #561 実 IME
3. **v0.6.0 安定版リリース**（ユーザー決定済み。#287 レビュー GO + スマホ実機確認 07-27 済み）:
   CHANGELOG 整理（nightly 0.5.10〜14 分を [0.6.0] へ日英併記）→ Cargo.toml 0.6.0 →
   `release.sh --publish` → homebrew-tako cask → install
4. リリースノート運用は #594 の新機構（ダウンロード表 / Known limitations / タグ規約）を初適用

## 未着手・持ち越し

- #601 案 2（外部ターミナル向け PATH 設置）= FR-2.14.5 / #608（表示言語グローバル競合フレーク）/
  #592（Windows watch 検知）/ #583（Windows テスト 19 件）
- キュー: #519 ⑤⑥ / #513 / #542 / #541 Phase 2 / Windows #467→#517
- セルフテストは worker 並走の高負荷（load 16〜32）でフレークしやすい（#212 系。単独再実行で緑）

## GUI 検証の環境知見（次回の時間浪費を防ぐ）

- **蓋閉じ（外部ディスプレイ無し）では GUI 検証不可**。`ioreg -r -k AppleClamshellState` を最初に確認
- **画面ロック / スクリーンセーバー中も不可**: `screencapture` が黒画かロック画面になり、
  `count of windows` = 0 になる（プロセス生死とは無関係。#549 検証で判明）
- **`cargo test` は起動用バイナリを更新しない**: GUI 実測の前に `cargo build` が要る
  （#549 検証で偽の不具合を 1 ラウンド追った）
- **`open` は呼び出し元シェルの env を継承する**: master ペインからの再起動は
  `env -u CLAUDE_CONFIG_DIR open -a tako` が必須（素の open で univ が混入 = #571 の再現条件）
- 蓋が開いていれば `cliclick` 実クリック + `screencapture -R` で実機検証可。合成キーボード入力は
  IME に吸われるため不可（キー検証はセルフテストで）
- この機には日本語入力ソースが無い（IME 実変換の検証はユーザー実機のみ）
- **`git stash` はリポジトリ共有**（worktree でも）。退避は `git diff > patch` を使う
- 本番 tako と隔離インスタンスの並走時、クリック前に CGWindowList で最前面を確認する

## 現フェーズで Read すべき設計書

- リリース作業: `scripts/release.sh`（#594 で `--notes-only` / `--update-notes` 追加）+ `CHANGELOG.md`
- Windows 移植を進める: `.agent/plans/2026-07-windows-port-architecture.md` + `.agent/windows-setup.md`
- 検知系に手を入れる: `crates/tako-control/src/claude_tui.rs` + `dispatch.rs` + `orchestrator/wait.rs`
  （#571/#572/#577 で画面判定・キュー検知・permission 実在検査が入った）
