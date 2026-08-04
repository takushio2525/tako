# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-04、Issue #750 MCP 全体リファクタ）

- Issue #750 へ着手コメントと、棚卸し表・対応方針・実施順序をコメント済み
- 先行 PR #751（#749）/ #753（#748）は main へ squash merge 済み
- PR #752 で 133 ツールの完全カタログスナップショットを先行導入し main へ merge 済み
- `refactor/750-mcp-structure` で `mcp.rs` を `mcp/{mod,catalog,request,http,tests}.rs` へ分割中
- 公開 API、133 ツール、スキーマ、順序、応答形式、protocol / dispatch / CLI は変更しない
- 全カタログ名が Request 変換または明示 special handler へ到達する網羅テストを追加済み

## 検証状況

- #748 merge 後の完全スナップショット SHA-256:
  `de522af54b1628270b23df8ca787ee8be64a23d74189933ff4ad1bf5cb09d7b3`
- 構造分割後も同一ハッシュ、MCP 単体 39 本・完全スナップショット 3 本は全緑
- `cargo fmt --all --check` / Clippy（全 target・warning deny）/ workspace test は全緑
- `TAKO_ISOLATED=1 TAKO_SELF_TEST=1 cargo run -p tako-app` は
  `TAKO_APP_SELF_TEST_OK`・exit 0 で完走
- 次: PR（Closes #750）→ macOS / Windows CI → squash merge → main 同期 / worktree 掃除 / install

## 不変条件

- 本番 GUI・本番 tmux に触れない。セルフテストは `TAKO_ISOLATED=1` を必須とする
- 挙動変更候補は実装せず Issue #750 の提案へ回す
- tako-core API → protocol / dispatch → CLI / MCP の 1:1 を維持する

## 未着手・持ち越し

- #691 GUI モードのクローズはユーザーの実使用確認待ち
- #658、#601 案 2、#632、#633、#638、#651 ほか既存キューは #750 の対象外
