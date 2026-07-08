# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-08・#111 tako solo 実装完了 / PR 前）

**#111「tako solo（オーケストレーション無しの 1 対 1 対話モード）」の WIP を仕上げて完了**。
前任 WIP は mod.rs 側（solo ロジック + テスト）と solo_system_prompt.md が完成済みだったが、
**CLI 側に solo コマンドが存在せず**、代わりに別機能（sessions）の未定義型を参照する
半焼け断片が混入してビルド不能だった。これを切り分けて対応:

- solo CLI コマンドを新規実装（`orchestrator_solo`）。master と対称・`build_master_claude_cmd` 共用。
  `tako solo [-profile]`、role/env は `solo` / `solo:<suffix>`、既定 effort=high、solo-profiles/ 分離
- sessions 断片を除去してビルド回復（#111 と無関係・**保全コミット `9783c33` に保存済み**、
  復元可能。別 issue で復活可）。tako-app のツール数を 52 へ戻す（solo は MCP 非追加）
- build/fmt/clippy/test 全緑。実バイナリで solo コマンド構築・role・effort=high・prompt 注入・
  エッジ 2 件（空プロファイル名 / 不在プロファイル）を検証済み
- フィーチャーコミット `99a1f4c`（`Refs #111`）。**push + PR は未実施 = 次アクション**

## 残作業・既知の制約

- **次アクション: push → PR 作成（本文 `Refs #111`。実対話は master/ユーザー確認のため `Closes` にしない）**
- squash merge と `build-app.sh --install` は **master の検収後に別途指示**（勝手に実行しない）
- 稼働中 tako（pid 45457）は旧バイナリ。solo を GUI から使うには再起動が必要（ユーザー作業）
- solo の実 claude 対話（信頼ダイアログ・実挙動）は未検証（トークン節約のため起動 IPC 手前まで確認）
- main.rs は 9,800 行前後。さらなる分割は別タスク
- CI（GitHub Actions）は 6/12 以降停止中（無料枠逼迫）。品質保証はローカル全緑で代替

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- solo / オーケストレーター修正時: `.agent/orchestrator.md` +
  `crates/tako-control/src/orchestrator/mod.rs`（solo_* 関数群）+ `solo_system_prompt.md`
- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
- リモート PWA 修正時: `web/tako-remote/src/pages/terminal.jsx` 冒頭コメント
