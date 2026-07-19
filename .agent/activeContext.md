# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-19・#381 完了 → #380 着手）

**Issue #380: タブバーの全ウィンドウ共有化** — #381（PR #400 squash merge 済み `f9c2ac8`）の
続きとして同一 worker が実装中。branch `fix/380-shared-tabbar`。

### #380 の要件（Issue 本文が正）
1. タブバーは全ウィンドウで共有（New Window 直後から既存タブが全部見える）
2. タブクリックでそのウィンドウへ表示が自動移動（内部は move_tab_to_window。排他原則維持）
3. 他ウィンドウで表示中のタブは区別表示（GPUI プリミティブ、絵文字禁止）
4. `tako window move-tab` CLI/MCP 互換維持
5. #339 受け入れ条件の回帰なし

## 次の一手

- tab_bar.rs の表示フィルタ（window_tab_ids）を全タブ列挙へ + クリックの表示奪取 + 区別バッジ
- cmd+数字 / next-prev タブの巡回も全タブ基準へ
- **修正版 install の推奨（ユーザー向け）**: #381 修正込みビルドを `build-app.sh --install` で
  反映するまで、旧ビルドでの「最後のウィンドウ赤ボタン close → Dock 復帰」はゾンビを増やす
- main のセルフテスト「worker_status IPC」失敗（素の main で再現、#381 と無関係）の扱いは master 判断

## 現フェーズで Read すべき設計書

- `.agent/architecture.md`「複数ウィンドウ」節（あれば）+ crates/tako-app/src/tab_bar.rs
