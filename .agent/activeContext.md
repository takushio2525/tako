# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-05・Issue #64 完了）

PC 版描画で半角文字が確率的に消える問題（#64）を根治し PR #70 squash merge 済み。
根因 = 半角グループ div の幅を GPUI が wrap_width として扱い、シェイプ幅の f32 ヘアライン
超過で末尾の単語/文字が行 div の overflow_hidden 外へ折り返されて不可視化（実測プローブで
確定。純 ASCII の「UI」「Fable 5 + max」でも幅が完全一致表示のまま wrap が発生）。

## #64 の修正構成（crates/tako-app/src/main.rs）

- 行 div に `whitespace_nowrap()` — GPUI の折り返し経路（wrap_width）を構造的に禁止（根治）
- `glyph_snaps_to_cell()`（advance 実測 + char 単位キャッシュ）でセル幅不一致グリフ
  （⏺ ⎿ 等のフォールバックフォント記号）をグループから除外し個別 div 化（ずれ累積の遮断）。
  ASCII は無条件グループ化なので #39 のハング解消効果（描画要素数削減）は維持
- グループ分割は純関数 `chunk_line_chars`（unit test 5 本）+ セルフテスト 69b（根因実在・
  nowrap 構造・グリフ隔離の 3 検査）。zed の `apply_force_width_to_layout` と比較裏取り済み

## リモート UI の現行構成（#63）

- **PC 非破壊**: WS の cols/rows 自動リサイズを全廃。`/ws?pane=<id>` は読み取り専用で
  ペインサイズに影響する経路が存在しない。REST `POST /resize` は CLI / MCP 用の明示操作として存置
- **WS プロトコル**: 接続時 `init`（履歴 2000 行 + 現画面 + カーソル、ANSI 付き）→
  250ms 差分で `update`。clear / サイズ変更 / 大量出力時は init 再送
- **クライアント**: xterm.js 廃止。折り返しリーダービュー + 自前 ANSI SGR パーサ
  （`web/tako-remote/src/ansi.js`）。フォント A−/A＋、ペイン切替はスワイプ + ヘッダー ‹ ›
- **検証用**: `TAKO_REMOTE_STATE_DIR` で pid/token/port を隔離、検証デーモンを本番と並走可能

## 残作業・既知の制約

- **スマホ実機テスト未実施**（#63 リーダービュー: タッチスクロール・慣性・選択・ソフトキーボード）
- main.rs は 9,801 行（#64 で検証コード込み +370 行）。さらなる分割は別タスク
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ。
  項目 46「全角行のクリック」は #37 修正済みで通過する（旧記述を訂正）
- CI（GitHub Actions）が 6/12 以降トリガーされていない（直近 PR はすべて CI なしでマージ）— 要調査

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- ターミナル描画（グループ化 / グリフ整合）修正時: `crates/tako-app/src/main.rs` の
  `chunk_line_chars` / `terminal_screen_lines` 周辺コメント（#39 / #64 の設計の正）
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント（API 仕様の正）
- リモート PWA 修正時: `web/tako-remote/src/pages/terminal.jsx` 冒頭コメント（リーダービュー設計）
- オーケストレーター修正時: `docs/orchestrator.md`
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
