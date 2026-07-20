# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-19・main 統合同期完了）

**統合ブランチ `renewal/remote-transport` に origin/main（23cbb59 まで 47 コミット）をマージ済み**

- remote 刷新（弾 3〜7、#282〜#287）は実装・検証完了（経緯は progress.md / 各 Issue）
- main 側の #391（setup 対話復元）/ #358（ツールカタログのスナップショット検証化）/
  #384（fail-safe PID 検証）/ #392（失敗トースト撤去）等を取り込み
- コンフリクト解決の要点: remote.rs = renewal 全面刷新が正 + main の #384/#330 を統合、
  setup.rs = #391 の対話起動と remote setup 案内を両立、
  changes.yaml = main の rev 11 を維持し Tailscale エントリを rev 12 へ振り直し

## 次の一手

- iPhone 実機確認（main 最新修正 + remote 刷新を両方含むビルドで）
- #287 の master レビュー・main マージ判断（renewal → main の逆マージは次フェーズ）
- v0.6.0 リリース判断

## 現フェーズで Read すべき設計書

- 計画の正: `.agent/plans/tako-remote-plan.md` §6（弾 3〜7 は統合ブランチ）
- 弾 0 実測: `.agent/investigations/tailscale-serve-poc.md`（serve / identity / URL 固定性）
- 実装: `crates/tako-control/src/tailscale.rs` / `crates/tako-control/src/remote.rs`
