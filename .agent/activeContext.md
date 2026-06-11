# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: Phase 2（Layer 1 — CLI と環境変数注入）実装完了。次は Phase 3（内蔵 MCP サーバー）
- ステータス: Phase 3 未着手。CI（macOS / Windows）緑確認待ちは push 直後のみ
- 最終更新: 2026-06-11

## 直近の観点・指摘

- **操作ディスパッチは `tako-control::dispatch`（ControlHost trait）に一元化済み**。
  Phase 3 の MCP サーバーは新しい操作実装を書かず、この dispatch を呼ぶだけにする（設計原則 5）
- IPC ワイヤ形式: 1 行 1 JSON の JSON-RPC 2.0 サブセット + `token` フィールド
  （`crates/tako-control/src/protocol.rs` が正）。UDS は `$TMPDIR/tako-<pid>-<seq>.sock`（0600）
- `tako` CLI: split / send / focus / list / read / close / title / resize / equalize /
  tab new・select・move-pane。`--pane` 省略時は `TAKO_PANE_ID` で呼び出し元を自動特定
- 環境変数注入は tako-app の `spawn_session` で実施（PANE/TAB/SOCKET/TOKEN。MCP_URL は Phase 3）
- 安全制約: CLI からの close は「最後のタブの最後のペイン」を拒否（アプリ終了は UI のみ）
- セルフテストは 29 項目（1〜13: ターミナル基盤、14〜29: 制御プレーン e2e。
  ペイン内シェルから実 `tako` バイナリを叩いて検証する）
- Windows named pipe は Phase 6 の TODO（`architecture.md`「IPC トランスポート」節に検討事項）
- gpui ハマりどころ（font-kit feature 必須等）は `poc/README.md` / `architecture.md` 参照（必読）

## 現フェーズで Read すべき設計書

- Phase 3 着手時: `.agent/architecture.md`「Layer 2」「IPC トランスポート（Phase 2 実装メモ）」節と
  `requirements.md` FR-2.3 / FR-2.5 を Read してから実装。
  MCP は `tako-control` の protocol / dispatch を共有し、トランスポート（Streamable HTTP
  localhost が第一候補）と `TAKO_MCP_URL` 注入・Claude Code 設定ゼロ接続だけを足す

## 未解決・次の一手

- [ ] Phase 3: MCP サーバー内蔵（dispatch 共有）→ `TAKO_MCP_URL` 注入 → Claude Code 設定ゼロ
      接続の実証 → role / 状態表示 UI（FR-2.1.3〜2.1.4）
- [ ] Phase 1 残骨格: ドラッグでのペイン境界リサイズ・IME 変換中表示は未実装（常用しながら判断）
- [ ] Phase 5 送り: Web ビューペイン（FR-3.8）・注釈レイヤ（FR-2.6）・diff ビューア（FR-3.9）・
      提示系（FR-2.7）・フィードバック（FR-2.8）・cmd+K（FR-2.9）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- PoC: `poc/`（Phase 0 検証コード。01: crates.io 版、02: git 版、03: 最小ターミナル）
