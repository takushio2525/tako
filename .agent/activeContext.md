# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-27 夜・v0.6.0 安定版リリース完了）

- tag `v0.6.0`（annotated）+ GitHub Release = **Latest**（prerelease/draft = false）。
  アセット `tako-v0.6.0-macos-arm64.zip`（16,283,465 B / sha256 `33bad2e0…`）
  https://github.com/takushio2525/tako/releases/tag/v0.6.0
- CHANGELOG に `[0.6.0]` を新設（v0.5.9 以降 = nightly 0.5.10〜0.5.13 + 未リリース分 +
  07-27 の 12 件を日英併記で統合）。`[Unreleased]` は空。main = `29837da`
- homebrew-tako cask 0.5.9 → 0.6.0（`acf412e`）。`brew fetch --cask` で sha256 実検証済み
- `/Applications/tako.app` = 0.6.0（CLI も 0.6.0）。**本番 GUI プロセスは 0.5.13 のまま**
- リリースノートは #594 の新機構を初適用（ダウンロード表 + macOS 手順を実アセットから生成）

## 次の一手

1. **GUI 再起動**（必ず `env -u CLAUDE_CONFIG_DIR open -a /Applications/tako.app`）→ 0.6.0 反映
2. 再起動後に `tako update check` が `{"available": false}` になることを本番でも確認
   （0.6.0 の隔離インスタンスでは検証済み）
3. ユーザー目視: 初回バナー(#549)/リモートインジケータ(#590)/入力予測(#600)/ツリー線(#589)/
   #562 マージ導線 / #496 カード / #561 実 IME
4. #434 の宣伝タスク（紹介動画 v3・README・docs の v0.6.0 追従）

## 未着手・持ち越し

- **Known limitations (Windows) 節はノートに出ていない**（#594 の設計どおり Windows
  アセットがあるときだけ付く）。Windows 版を同タグに後付けしたら
  `scripts/release.sh --update-notes v0.6.0` で作り直す
- cask の caveats が「未署名のため」と書いているが実際は Apple Development 署名済み
  （Developer ID / notarization が無いだけ）。文面の是正は未着手
- #601 案 2（外部ターミナル向け PATH 設置）= FR-2.14.5 / #608（表示言語グローバル競合フレーク）/
  #592（Windows watch 検知）/ #583（Windows テスト 19 件）
- キュー: #519 ⑤⑥ / #513 / #542 / #541 Phase 2 / Windows #467→#517

## GUI 検証の環境知見（次回の時間浪費を防ぐ）

- **蓋閉じ（外部ディスプレイ無し）では GUI 検証不可**。`ioreg -r -k AppleClamshellState` を最初に確認
- **画面ロック / スクリーンセーバー中も不可**: `screencapture` が黒画かロック画面になり、
  `count of windows` = 0 になる（プロセス生死とは無関係。#549 検証で判明）
- **`cargo test` は起動用バイナリを更新しない**: GUI 実測の前に `cargo build` が要る
- **`open` は呼び出し元シェルの env を継承する**: master ペインからの再起動は
  `env -u CLAUDE_CONFIG_DIR open -a tako` が必須（素の open で univ が混入 = #571 の再現条件）
- **隔離インスタンスへの CLI 接続**: `TAKO_ISOLATED=1` の discovery は
  `$TMPDIR/tako-iso-discovery-<pid>/control.json`。そこの socket / token を
  `TAKO_SOCKET` / `TAKO_TOKEN` に渡すと本番に触れずに dispatch を叩ける
  （v0.6.0 の「更新なし」検証はこの方法で実施）
- **`git stash` はリポジトリ共有**（worktree でも）。退避は `git diff > patch` を使う

## 現フェーズで Read すべき設計書

- リリース作業: `scripts/release.sh`（`--notes-only` / `--update-notes`）+ `CHANGELOG.md` +
  `.agent/conventions.md`「CHANGELOG / リリースノートのプラットフォーム表記」
- Windows 移植を進める: `.agent/plans/2026-07-windows-port-architecture.md` + `.agent/windows-setup.md`
- 検知系に手を入れる: `crates/tako-control/src/claude_tui.rs` + `dispatch.rs` + `orchestrator/wait.rs`
