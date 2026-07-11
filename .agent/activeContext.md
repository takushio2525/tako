# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-11・#127 master codex 対応 merge 済み）

#127 オーケストレーション master の codex 対応が完了
（PR #128 squash merge `954330c` + build-app.sh --install 済み）。

- プロファイル `master_agent: codex` + model / effort（ネイティブ表記）で codex master / solo を起動
- system prompt は `-c developer_instructions="$(cat …)"`、MCP は `-c mcp_servers.tako.*` 一時注入
  （env_vars で TAKO_* を stdio ブリッジへ引き継ぎ。config.toml は汚さない）
- 波及ガード: master_agent≠claude のとき model / effort を claude worker へ継承しない
- agy は master 非対応（明示エラー。worker のみ）
- 実 e2e 済み: codex master 起動 → /mcp で tako 全 53 ツール列挙を確認

## 残作業・既知の制約

- **ユーザーの sol プロファイル作成は未実施**（機能のみ提供。`tako orchestrator profiles set sol
  --master-agent codex --model gpt-5.6-sol --effort xhigh` + worker 設定で作れる）
- codex master への実プロンプト送信（MCP ツール実呼び出し）は codex 利用上限（7/11 20:40 解除）
  のため未検証。ツール列挙まで実証済み
- GUI テキスト選択（#124）の実機検証が未了（ユーザーによるマウスドラッグ操作が必要）
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ
- CI（GitHub Actions）は 6/12 以降停止中（無料枠逼迫）。品質保証はローカル全緑で代替

## 未着手タスク（優先順はユーザーと相談）

- [ ] **#115 GitLog / GitDiff dispatch の background 化**
- [ ] **#116 tako-coretest-* ソケット残骸の掃除・再発防止**
- [ ] **#112 セッション会話ログの管理と復元**
- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- solo / オーケストレーター修正時: `.agent/orchestrator.md`（master_agent の設計は
  「master のエージェント種別」節 + `crates/tako-control/src/orchestrator/mod.rs` の build_master_cmd）
- PDF プレビュー修正時: `crates/tako-app/src/preview.rs` の `pdf_render` モジュール + `preview_render.rs` の PDF body 生成
- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
