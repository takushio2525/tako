# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-08・#113 修正 merge 済み・実機反映は tako 再起動待ち）

**#113（多ペイン並列時のフリーズ + 強制終了後のペイン消失）の修正を PR #114 で squash merge**。
ペイン消失は根治: 多重インスタンスガード（セカンダリモード、FR-5.8）+ 起動時 orphan cleanup の
activity 1 時間猶予（FR-2.16.11 第四ガード）+「全ペイン終了」二重発火の冪等化。
フリーズは単一根因未特定のため診断を導入（UI ストールウォッチドッグ + dispatch 遅延計測 →
`<data_dir>/perf.log`）+ 実証済みブロック源（tmux window capture の UI 同期実行）を除去。

- build-app.sh --install 済み（**反映は tako 再起動後**）
- 再起動後の実機確認: ①2 個目の起動がセカンダリモード（persist.log に「復元スキップ」）になる
  ②通常の再起動復元が従来どおり動く → 確認できたら #113 を close（master 判断）
- フリーズが再発したら `<data_dir>/perf.log` を見る（犯人の dispatch 種別と UI 専有時間が残る）

---

## 残作業・既知の制約

- main.rs は 9,900 行前後。さらなる分割は別タスク
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ
- CI（GitHub Actions）が 6/12 以降トリガーされていない — Actions 無料枠逼迫で停止中
- cask 0.3.2 更新は未実施（`homebrew-tako` 側）
- KV の eventual consistency でレートリミットのバースト耐性は緩い（1 分窓では効く）
- 多重インスタンスガードは macOS のみ（Windows は Phase 6）。`TAKO_FORCE_PRIMARY=1` で無効化可

## 未着手タスク（優先順はユーザーと相談）

- [ ] **#115 GitLog / GitDiff dispatch の background 化**（zed 級リポで 2431ms UI 専有の実測あり）
- [ ] **#116 tako-coretest-* ソケット残骸の掃除・再発防止**（/tmp に 2,791 個堆積）
- [ ] **#111 solo コマンド**（メイン working tree の feature/111-solo-command に WIP あり）
- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
- リモート PWA 修正時: `web/tako-remote/src/pages/terminal.jsx` 冒頭コメント
- オーケストレーター修正時: `.agent/orchestrator.md`（品質パイプラインの表）+
  `crates/tako-control/src/orchestrator/default_system_prompt.md`
- persist / 多重起動 / cleanup 修正時: `.agent/requirements.md` FR-5.8 / FR-2.16.11 +
  `crates/tako-app/src/main.rs` の `TakoApp::new` 冒頭（セカンダリモード判定）
