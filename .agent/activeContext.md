# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・v0.4.0 リリース済み + #169 projects.yaml 全消失の根治）

**v0.4.0 リリース済み**（tag `v0.4.0` = `98b17ea`、バイナリ付き GitHub Release、
Pages デプロイ、homebrew-tako cask 0.4.0 更新済み）。夜間リリースは launchd
ローカルジョブへ移行（#166 / PR #170。`com.takushio.tako-nightly-release` 毎日 5:00）。

#169（orchestrator projects.yaml が並行 add で 58 件 → 1 件に全消失）を根本修正。
根本原因は三段連鎖: ①旧 save = `std::fs::write`（truncate → write の窓で並行プロセスに
空 / 部分ファイルが見える）②serde_yaml が空 / 部分内容を「0 件」として**成功**パース
（`#[serde(default)]` + 空 = null。エラーにならない）③read-modify-write のプロセス間
直列化なし（GUI の MCP dispatch と CLI が別プロセス）。

- 新設 `tako-control::config_io`: アトミック書き込み（tmp + fsync + rename）/
  `<path>.lock` 排他 flock（std `File::lock`）/ `.bak.1`〜`.bak.3` 世代バックアップ
- `ProjectsConfig::mutate`・`Profile::mutate_named`・`setup::mutate_config` で
  ロック付き RMW に統一。パース失敗時は**一切書き込まず Err**（fail-loud）
- 横展開: profiles set の `unwrap_or_default()` 握りつぶし修正、config.yaml の RMW
  ロック化、`ensure_defaults` の TOCTOU 解消
- 詳細は `.agent/architecture.md`「設定ファイル I/O の安全化」節

## 検証済み（#169）

- workspace build / test（507 tests）/ fmt / clippy（-D warnings）全緑
- 根本原因の実証テスト 2 本（空 YAML の 0 件成功パース / truncate 窓での 58→1 再現）
- 実機 before/after: 修正前バイナリ = 並行 add 60 件で 48 件消失を再現 →
  修正後 = 118/118 全件残存 + bak 3 世代生成 + 破損 YAML add 拒否（exit=1・ファイル不変）+
  bak.1 からの復元 → add 再開成功（すべて隔離 HOME、本物の projects.yaml 不使用）

## 次の一手

- #169: squash merge → `build-app.sh --install` → Issue クローズ
- 明朝 5:00 の夜間ジョブ初回実行を監視: main が v0.4.0 タグより先行しているため
  v0.4.1 が自動リリースされる見込み = 全経路の初通し検証
- tako 再起動後の GUI 確認（manual-checks.md）: 「Web ビューペイン」節（#155）、
  「#153 節」（cmd ホバー装飾）、「#152 節」（PDF 選択・色分け）+ Cmd-Q 経過観察（#103）
- Phase 5 の次候補は FR-2.19 localhost ポートパネル・FR-3.10 画像プレビュー等

## 現フェーズで Read すべき設計書

- 設定ファイル I/O の安全化（#169）: `.agent/architecture.md` 該当節
- Web ビュー実装詳細と z オーダー制約: `.agent/architecture.md`「Web ビューペイン」節
