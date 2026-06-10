# conventions.md — 規約（命名・エラー・ログ）

> 仕様策定フェーズの最小版。コード着手（Phase 0〜1）で実態に合わせて拡充する。

## 命名規則（Rust 標準に従う）

| 対象 | 規則 | 例 |
|---|---|---|
| クレート | kebab-case、`tako-` 接頭辞 | `tako-core`, `tako-cli` |
| モジュール / 関数 / ファイル | snake_case | `pane_tree.rs`, `split_pane()` |
| 型 / trait | PascalCase | `PaneTree`, `TerminalSession` |
| 定数 | SCREAMING_SNAKE_CASE | `DEFAULT_SCROLLBACK` |
| 環境変数 | `TAKO_` 接頭辞 | `TAKO_PANE_ID` |
| CLI サブコマンド | 小文字 1 単語 | `tako split` |
| MCP ツール | `tako_` 接頭辞 + snake_case | `tako_split_pane` |

## エラーハンドリング

- ライブラリクレート（core / control）: `thiserror` で型付きエラー、`Result` を返す
- バイナリ（app / cli）: 境界で `anyhow` 可
- `unwrap()` / `expect()` は「論理的に到達不能」な場合のみ。理由をコメントに書く

## ログ

- `tracing` クレート。レベル: `debug` / `info` / `warn` / `error`
- **ペイン内容・送信テキスト・`TAKO_TOKEN` をログに書かない**（ユーザーの入力・秘密情報を含むため）

## フォーマット / Lint

- `cargo fmt`（rustfmt デフォルト）+ `cargo clippy -- -D warnings` を CI で強制

## ドキュメント

- 仕様書は `.agent/`（日本語）。コードコメントも日本語
- 仕様変更時は該当する `.agent/*.md` を**同一コミット**で更新する
