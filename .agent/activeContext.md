# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-14・インラインテキスト入力 UI 完成）

- **FR-3.12 インライン編集 UI 完成**: ファイルツリーのコンテキストメニューから「名前変更」
  「新しいファイル」「新しいフォルダ」を選ぶと、ツリーの該当行にインラインテキスト入力欄が
  表示される。Enter で確定（dispatch FileOp）、Esc でキャンセル。IME 入力にも対応
  （EntityInputHandler の replace_text_in_range をインライン編集中に振り分け）
- 新規ファイル/フォルダは親ディレクトリの子の末尾に仮行を挿入して入力。
  親が未展開なら自動展開する（`FileTree::expand_dir`）
- カーソル移動（←→Home/End）、BackSpace/Delete、IME 確定文字列の挿入をサポート
- MCP ツール数が 23 に更新（前回の FR-3.12 で `tako_file_op` 追加分。セルフテスト期待値修正）
- cargo test 88 pass・clippy / fmt 緑・`.app` 反映済み
- **ユーザーの再起動 + 実機確認待ち**
- 最終更新: 2026-06-14

## 残作業・既知の制約

- コンテキストメニューの位置がサイドバー基準でなくウィンドウ基準になる可能性
  （GPUI の `position` がウィンドウ座標のため。実機で確認してから調整）
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知。今回無関係）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.6 git graph / FR-3.5 軽い編集 / FR-3.9 diff ビューア
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）
- **Edit ツールのフックが変更を巻き戻す**: Bash + python3 での一括パッチが安全。
  複数ファイルにまたがる変更は Edit ツールではなく Bash で一括適用する
- **GPUI の ClickEvent.is_right_click()**: `on_click` のクロージャで右クリック判定可能
- **インライン編集 UI**: `handle_key` の冒頭で `inline_edit.is_some()` をチェックし、
  Enter/Esc/文字入力を横取り。IME 確定は `replace_text_in_range` で振り分け

## 現フェーズで Read すべき設計書

- FR-3.6 git graph 着手時: `architecture.md`「コンセプト②の実現」
- 配布・オンボーディング着手時: `roadmap.md` Phase 7

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
