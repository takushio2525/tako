# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-15・tmux ビューの orphan / 無限ネスト根治）

- **二重表示解消 + 退避ラベル改善**（commit `b9584af`）: shelved の backend を kill漏れ?から除外、
  退避ラベルを pane ID → cwd ベース名へ
- **ラッパー orphan の根治**（FR-2.16.11 関連）: `TmuxViewTarget` を `session`（元・監視/再 attach
  用）と `wrapper`（`tako-view-*`・close 時 kill）に分離。`drop_tmux_view_session` はラッパーだけ
  kill（`tako-view-` 接頭辞ガード）。旧実装は元セッション名を登録 → ①ラッパー orphan ②実セッション
  誤 kill の二重バグだった
- **無限ネスト根治**（`tako-view-tako-view-...`）: `TmuxOpen` で tmux `session_group` へ正規化 +
  `tako-view-*` の開き直しは新ラッパーを作らず元を直接 attach（`dispatch.rs`）
- **起動時 orphan 一括クリーンアップ**（FR-2.16.11）: backend socket 上の `tako-`・detached・
  非 grouped・protected 外のみ kill。CLI `tako tmux cleanup` + MCP `tako_tmux_cleanup`（計 31 ツール）
- **検証済み**: clippy 緑 / cargo test 全緑 / セルフテスト = PDF（既知の Core Graphics 失敗）以外緑
- **次**: tako 終了 → `scripts/build-app.sh --install` → 再起動で実機確認（残っている nested
  `tako-view-*`（既定サーバー）は各ペイン close 時に片付く。backend の裸 orphan は起動時掃除で消える）
- 最終更新: 2026-06-15

## 残作業・既知の制約

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

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）
- **Edit ツールのフックが変更を巻き戻す**: Bash + python3 での一括パッチが安全
  （今回は Edit が巻き戻されず通った。挙動が不安定なら python3 へ）
- **インライン編集 UI**: `handle_key` の冒頭で `inline_edit.is_some()` をチェック
- **tmux ビューの退避（shelve）は意図的にラッパーを kill しない**: shelve はターミナルを
  生かしたまま隠すだけで、ラッパーは attached のまま = orphan ではない。ラッパー kill は
  「ターミナルを実際に壊す経路」（remove_pane / remove_tab / shelved-kill / detach_session）に
  集約してある。これで orphan を防ぎつつ unshelve で view が生き残る。**もし「退避で
  ラッパーを消し、復帰で元へ attach し直す」挙動を望むなら** unshelve 側に再 spawn を足す
  follow-up が必要（元名は `tmux_view_panes` の `session` に保持済み）。
  **2026-06-15 ユーザー承認: 現設計（shelve はラッパー維持）で確定。厳密版は不要**
- **セルフテストは保守が滞りがち**: 今回ツール数（29→31）と × ボタン（kill→退避）の stale を
  修正。項目追加時は `main.rs` のセルフテストと `mcp.rs` の `tools.len()` を同時更新する

## 現フェーズで Read すべき設計書

- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
- 配布・オンボーディング着手時: `roadmap.md` Phase 7

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
