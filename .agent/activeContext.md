# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: Phase 3（Layer 2 — 内蔵 MCP サーバー）コア完了。
  次は Phase 3.5（日常使い品質: IME 変換中表示 + .app バンドル）と
  Phase 3 残（role/状態表示 UI → Phase 4 の集約センターと併せて）
- ステータス: push 直後の CI（macOS / Windows）緑確認待ちのみ
- 最終更新: 2026-06-11

## 直近の観点・指摘

- **MCP は 2 トランスポート**: Streamable HTTP（tako-app 内蔵、`TAKO_MCP_URL` 注入、
  Bearer + Origin 検証、tiny_http）と stdio ブリッジ（`tako mcp serve`。実行を IPC へ
  origin="mcp" で中継）。エンジン `tako-control::mcp` は共有で、実行は dispatch を呼ぶだけ
- **Claude Code 設定ゼロの現実解**: 環境変数からの MCP 自動発見機構は無い（2.1.x で検証）。
  初回 1 回 `claude mcp add --scope user tako -- <repo>/target/debug/tako mcp serve` を登録
  → 以後どのペインでもゼロ設定。tako 外では 0 ツールで無害。
  実機検証は `scripts/verify-claude-mcp.sh`（実 claude で stdio / HTTP 両経路 OK、2026-06-11）
- ツールは 12 個（FR-2.5 と 1:1）。send / read は pane 必須（誤送信防止）、close は
  pane 省略 = 自己片付け。行動規範（FR-2.7.5）は initialize の instructions + 説明文に埋め込み
- セルフテストは 36 項目（30〜36 が MCP）。**セルフテスト future はメインスレッドで動くため、
  dispatch 往復を伴うブロッキング呼び出しは background executor へ逃がす**（デッドロックの教訓）
- ユーザーが tako の日常常用を開始予定 → Phase 3.5（FR-1.9 IME = Must、.app バンドル。
  アイコンは `assets/icon/icon-a.svg` 決定済み）。新規仕様: 画像プレビュー FR-3.10（Phase 5）、
  セッション永続性 FR-5（v0.2 以降）
- gpui ハマりどころ（font-kit feature 必須等）は `poc/README.md` / `architecture.md` 参照（必読）

## 現フェーズで Read すべき設計書

- Phase 3.5 着手時: `requirements.md` FR-1.9 と `architecture.md`「Phase 0 検証結果」
  （IME は GPUI の input handler まわり。gpui の ime 対応状況は要調査）
- Phase 4 着手時: `.agent/architecture.md`「Layer 3」節 + `requirements.md` FR-2.4 / FR-2.10 /
  FR-2.1.3〜2.1.4（role/状態表示 UI は Phase 3 残をここで回収）

## 未解決・次の一手

- [ ] Phase 3.5: IME 変換中表示（FR-1.9 = M）+ .app バンドル化（icns / Info.plist）
- [ ] Phase 3 残: role ラベル / 状態表示 UI（FR-2.1.3〜2.1.4。Phase 4 集約センターと併せて）
- [ ] Phase 4: パッシブ検知（OSC 7/133・listen ポート・提案チップ・集約センター）
- [ ] Phase 1 残骨格: ドラッグでのペイン境界リサイズ（常用しながら判断）
- [ ] Phase 5 送り: 画像プレビュー（FR-3.10）・Web ビュー（FR-3.8）・注釈（FR-2.6）・
      diff（FR-3.9）・提示系（FR-2.7）・フィードバック（FR-2.8）・cmd+K（FR-2.9）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- MCP 実機検証: `scripts/verify-claude-mcp.sh` / `crates/tako-control/examples/mcp_host.rs`
