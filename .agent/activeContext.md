# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-08・#113 修正 + #111 solo コマンド 両方 merge 済み）

直近クラッシュ（多ペイン並列でフリーズ → 強制終了でペイン消失）を **#113 で根治**し、
クラッシュ前に実装途中だった **#111 tako solo コマンドを仕上げて merge** した。

- **#113**（多重起動によるペイン消失）: 多重インスタンスガード（セカンダリモード FR-5.8）+
  起動時 orphan cleanup の activity 1 時間猶予（FR-2.16.11）+「全ペイン終了」二重発火の冪等化。
  フリーズは単一根因未特定のため診断を導入（UI ストールウォッチドッグ + dispatch 遅延計測 →
  `<data_dir>/perf.log`）+ 実証済みブロック源（tmux window capture の UI 同期）を除去。
  PR #114 merge（`fe73b60`）。**実機確認済み（20 匹スポーン負荷 OK・セカンダリモード確認）→ #113 close**
- **#111**（tako solo = オーケストレーション無しの 1 対 1 対話モード）: `tako solo [-profile]`、
  role/env `solo` / `solo:<suffix>`、既定 effort=high、worker spawn 禁止、solo-profiles/ 分離。
  前任 WIP に混入していた別機能 sessions 断片は保全コミット `9783c33` に退避。PR #117 squash merge。
  **実対話（信頼ダイアログ・GUI 実挙動）のみ未検証 = 再起動後に確認**

## 残作業・既知の制約

- **tako 再起動が必要**（稼働中プロセスは旧バイナリ。build-app.sh --install 済み）。
  再起動後に `tako solo` の実対話を確認 → #111 close
- sessions 断片（#112 会話ログ管理の書きかけ）は `9783c33` に保全。#112 再開時に復元可
- main.rs は 9,900 行前後。さらなる分割は別タスク
- 多重インスタンスガードは macOS のみ（Windows は Phase 6）。`TAKO_FORCE_PRIMARY=1` で無効化可
- フリーズが再発したら `<data_dir>/perf.log`（犯人の dispatch 種別と UI 専有時間が残る）
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ
- CI（GitHub Actions）は 6/12 以降停止中（無料枠逼迫）。品質保証はローカル全緑で代替
- cask 更新は未実施（`homebrew-tako` 側）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **#115 GitLog / GitDiff dispatch の background 化**（zed 級リポで 2431ms UI 専有の実測あり）
- [ ] **#116 tako-coretest-* ソケット残骸の掃除・再発防止**（/tmp に 2,791 個堆積）
- [ ] **#112 セッション会話ログの管理と復元**（sessions 断片が `9783c33` に保全済み）
- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- solo / オーケストレーター修正時: `.agent/orchestrator.md` +
  `crates/tako-control/src/orchestrator/mod.rs`（solo_* 関数群）+ `solo_system_prompt.md`
- persist / 多重起動 / cleanup 修正時: `.agent/requirements.md` FR-5.8 / FR-2.16.11 +
  `crates/tako-app/src/main.rs` の `TakoApp::new` 冒頭（セカンダリモード判定）
- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
- リモート PWA 修正時: `web/tako-remote/src/pages/terminal.jsx` 冒頭コメント
