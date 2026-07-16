# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#282 remote 刷新 弾3 = 統合ブランチ開始）

**ブランチ `renewal/remote-transport`（このブランチ）で Tailscale transport 一本化を実装済み**:

- `tako-control::tailscale` 新設: CLI 検出（brew / App Store）・setup 状態判定
  `setup_status()`（弾 6 ウィザードと共有）・serve start/stop/state・ts.net URL 解決
- remote daemon: cloudflared / Quick Tunnel / KV リレー / `--insecure` を全削除。
  start 時に setup 判定 → 不足列挙 + `tako remote setup` 誘導で停止 → serve 設定 →
  固定 ts.net URL を提示。stop / 異常終了時は**自分が設定した serve のみ**解除
- protocol / dispatch / host / tako-app / CLI / MCP から insecure を同期削除。
  PWA から relay 解決を削除。`web/tako-remote-worker/` 削除。
  setup 依存チェック（#88）から cloudflared 削除。docs / README 更新済み
- 検証で直した main 由来バグ: daemon_status が 3 行 PID 形式（#280）に未追従で常に
  running=false / spawn_daemon が PATH の旧 tako へ化ける / 子 stderr の握りつぶし

## 検証状態

- 品質ゲート（fmt / clippy -D warnings / build / test 843 本）+ 隔離セルフテスト完走
- 未 setup 4 状態（未導入 / デーモン未起動 / 未ログイン / HTTPS 未有効）の start 拒否を実測
- fake tailscale で start → serve 設定 → API 到達 → stop → serve off、SIGKILL 残骸再利用、
  別ポート残骸拒否、管理外 serve 保護、stop 二重実行を実測
- 実 tailnet 通し実測も達成（2026-07-17）: start → 固定 ts.net URL へ TLS 検証込み到達
  （health / PWA / 認証 API / WS 101）→ stop → 到達不能 + serve 設定ゼロ。
  受け入れ条件 1〜4 完了、証拠は Issue #282 コメント。iPhone 実機到達のみ要実機確認

## 次の一手

- 弾 4（#283 機器ペアリング認証 + PWA daemon 配信化）をこのブランチに積む

## 現フェーズで Read すべき設計書

- 計画の正: `.agent/plans/tako-remote-plan.md` §6（弾 3〜7 は統合ブランチ）
- 弾 0 実測: `.agent/investigations/tailscale-serve-poc.md`（serve / identity / URL 固定性）
- 実装: `crates/tako-control/src/tailscale.rs` / `crates/tako-control/src/remote.rs`
