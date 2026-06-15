# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-15・退避 UI 刷新 + ×=kill バグ修正）

- **緊急バグ修正（commit `16066b5`）**: ペインの × が `shelve_pane` だけを呼び、
  `drop_tmux_view_session` / `drop_backend_session` を呼ばず、tmux セッション（backend
  `tako-*` / view `tako-view-*` ラッパー）が kill されず「管理外」に残っていた。× を
  `remove_pane`（tree close + terminal/preview 削除 + 両セッション kill、LastPane はタブごと）
  へ変更し、タブの × と挙動統一。**ペイン単位の退避動線は失わないよう、タブと同じ
  ー（退避 = `shelve_pane_button`）ボタンをペインタイトルバーに追加**
- **退避エリア刷新（commit `9791b6a`）**: たまり場ドロワーを縦テキストリスト →
  **横並びの実画面プレビューカード + 横スクロール**へ。各カードは通常ペインの行描画を
  共通化した `terminal_screen_lines()` で実画面サムネイルを描き（カード本文サイズへ
  退避ターミナルを resize）、上部に通常ペイン同様のタイトルバー（状態ドット + ラベル +
  「復帰」+ 右上 × kill）。× は「完全に破棄?」確認 → tmux セッションごと kill
- **検証済み**: clippy 緑 / fmt 緑 / セルフテスト = PDF（既知の Core Graphics 失敗）以外緑
  （47=× kill / 47b=ー 退避 / 47c=ドロワー横並びプレビュー描画 を追加・通過）
- **次**: tako 終了 → `scripts/build-app.sh --install` → 再起動で実機確認（push 済み）
- 最終更新: 2026-06-15

## 残作業・既知の制約

- ドロワーは現状リサイズ不可（既定 240px 固定。必要なら上端ドラッグ handle を follow-up）
- 退避にプレビューペイン（ターミナル無し）が含まれる場合カード本文はラベルのみ表示
- コンテキストメニューの位置がサイドバー基準でなくウィンドウ基準になる可能性
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知）
- git パネルのコミットグラフは現在テキストベース

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。fmt 漏れがそのまま
  入る（今回 tmux.rs:136 の既存崩れを `da26023` で修正）。コミット前は必ず
  `cargo fmt --all --check`（exit code）も確認する。パイプ + `&& echo OK` は `tail` の
  exit を見てしまい誤判定するので注意
- **× と ー の役割（2026-06-15 確定）**: ペイン / タブとも ー = 退避、× = kill。
  × は `remove_pane`（= `tako close` 相当で CLI/MCP と動線一致）、ー は `shelve_pane`
  （= `tako shelve` 相当）。開発不変条件は両方とも既存 dispatch にマップ済みで満たす
- **退避プレビューは退避ターミナルを resize する**: カード本文 cols/rows に合わせて
  `resize`（冪等）。復帰時は render_pane が元サイズへ戻す。TUI は SIGWINCH で再描画
- **Edit ツールのフックが変更を巻き戻す**: 大きい置換は Bash + python3 が安全
  （今回 render_drawer は python3 一括置換で通した）
- **セルフテストは保守が滞りがち**: 項目追加時は `main.rs` のセルフテストと
  `mcp.rs` の `tools.len()` を同時更新（今回 MCP ツール数の変化なし）

## 現フェーズで Read すべき設計書

- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
- 配布・オンボーディング着手時: `roadmap.md` Phase 7

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
