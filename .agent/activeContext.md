# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-21・main 統合同期完了）

**統合ブランチ `renewal/remote-transport` に origin/main（#380 完了 = 498a811 まで）をマージ済み**

- remote 刷新（弾 3〜7、#282〜#287）は実装・検証完了（経緯は progress.md / 各 Issue）
- main 側の #391（setup 対話復元）/ #358（ツールカタログのスナップショット検証化）/
  #384（fail-safe PID 検証）/ #392（失敗トースト撤去）/ #381（Dock 復帰の全タブ消失根治）/
  #380（タブバー全ウィンドウ共有）/ #399（Finder D&D）等を取り込み。
  MCP ツールカタログは 103（+tako_remote_setup / tako_remote_devices）
- コンフリクト解決の要点: remote.rs = renewal 全面刷新が正 + main の #384/#330 を統合、
  setup.rs = #391 の対話起動と remote setup 案内を両立、
  changes.yaml = main の rev 11 を維持し Tailscale エントリを rev 12 へ振り直し
- 既知: セルフテスト「worker_status IPC（#181）」失敗は**素の main で再現する main 由来の
  問題**（#381 worker 確認、#390 worker が対処中。マージ起因ではない）

## 次の一手

- iPhone 実機確認（main 最新修正 + remote 刷新を両方含むビルドで）
- #287 の master レビュー・main マージ判断（renewal → main の逆マージは次フェーズ)
- v0.6.0 リリース判断（#381 + #380 + 並行 worker 分同梱）

## 現フェーズで Read すべき設計書

- 計画の正: `.agent/plans/tako-remote-plan.md` §6（弾 3〜7 は統合ブランチ）
- 弾 0 実測: `.agent/investigations/tailscale-serve-poc.md`（serve / identity / URL 固定性）
- 実装: `crates/tako-control/src/tailscale.rs` / `crates/tako-control/src/remote.rs`
