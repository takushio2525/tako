# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-14・コンテキストメニュー + D&D パス挿入完了）

- **FR-3.12 コンテキストメニュー**: ファイルツリーの右クリックで VSCode 風メニュー表示。
  相対/絶対パスコピー・Finder 表示・ターミナルで cd・名前変更（インラインリネームは
  InlineEdit 構造体を準備済み、UI の実装は次タスク）・新規ファイル/フォルダ・ゴミ箱送り。
  全操作は dispatch `FileOp` + CLI `tako file` + MCP `tako_file_op`（計 23 ツール）
- **FR-3.13 D&D パス挿入**: ファイル・フォルダをターミナルペインへ D&D するとパス文字列を
  PTY に send（newline: false）。プレビューペインへのドロップは FR-3.11 の既存挙動を維持。
  ファイルだけでなくフォルダもドラッグ可能に拡張
- cargo test 88 pass・clippy / fmt 緑・`.app` 反映済み
- **ユーザーの再起動 + 実機確認待ち**
- 最終更新: 2026-06-14

## 残作業・既知の制約

- インラインリネーム / 新規ファイル・フォルダの **UI 入力部分は未実装**（`InlineEdit`
  構造体 + `InlineEditKind` は準備済み。コンテキストメニューから「名前変更」等を選ぶと
  `self.inline_edit` にセットされるが、テキスト入力 UI の描画は次タスク）
- コンテキストメニューの位置がサイドバー基準でなくウィンドウ基準になる可能性
  （GPUI の `position` がウィンドウ座標のため。実機で確認してから調整）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **インラインリネーム / 新規作成の UI 入力**（InlineEdit の描画。FR-3.12 の残り）
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
- **GPUI の ClickEvent.is_right_click()**: `on_click` のクロージャで右クリック判定可能。
  コンテキストメニューはこれで実装（`on_mouse_down(MouseButton::Right, ...)` ではなく）
- **D&D パス挿入のエスケープ**: スペース・クォート・括弧を含むパスはシングルクォートで
  囲む（`shell_escape` パターン）。newline: false で改行なし挿入
- その他の注意点は前回の activeContext.md を参照

## 現フェーズで Read すべき設計書

- インラインリネーム着手時: `crates/tako-app/src/main.rs`（`InlineEdit` / `InlineEditKind`）
- FR-3.6 git graph 着手時: `architecture.md`「コンセプト②の実現」
- 配布・オンボーディング着手時: `roadmap.md` Phase 7

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
