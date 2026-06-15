# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-15・タブツリー ホバー/ピン プレビュー = 4 機能完了）

f64d2a3 の続きでタブツリー UI に 4 機能を追加（全 QA 緑、未プッシュ → push 予定）。
コミット 4 本: `765af0d`(F1) / `cf04a31`(F2) / `96d01b3`(F4) / `c12d4c5`(F3)。

- **F1 ホバープレビュー（FR-2.16.13）**: バックグラウンド行（`!is_active`）を `on_hover` で
  マウス位置の左にポップアップ表示。`terminal_screen_lines` を**リサイズせず**読むので
  バックグラウンドのプログラムを乱さず、`on_term_event` の notify で**ライブ更新**。読取専用
- **F2 折りたたみ改修（FR-2.16.14）**: 意味論を「全部隠す」→「**バックグラウンド行+退避だけ
  隠し前面行は残す**」（Q2 ユーザー選択）。`collapsed_tmux_tabs` を `group_index`→`TabId` キー化。
  `Request::CollapseTab` + `tako collapse` + MCP `tako_collapse_tab` + list の `collapsed` +
  layout.json 永続化
- **F4 グループプレビュー（FR-2.16.16）**: 閉じたタブグループカードを `on_hover` で全退避ペインを
  縦積みプレビュー。`PreviewTarget::ClosedGroup(TabId)`、`ClosedOriginShelfGroup.tab` 追加。
  ※閉じタブごとの分割自体は f64d2a3（FR-2.15.6）で実装済み
- **F3 ピン留め（FR-2.16.15）**: 📌 ボタンで**アプリ内フローティングウィンドウ**化（OS マルチ
  ウィンドウ不使用＝Windows 必須要件のため）。`pinned_previews` + `dragging_pin`（既存マウス
  経路統合）+ タイトルバー D&D 移動 + × 解除。`Request::Pin`（pane/group_tab 排他・省略でトグル）
  + `tako pin` + MCP `tako_pin_preview` + list の `pinned`。ランタイム状態（再起動はまたがない）
- **検証済み**: build / clippy(-D warnings, exit 0) / fmt / test 全緑（app33 / cli10 /
  control58 / core103）。セルフテストは項目70 PDF（既知 Core Graphics）以外緑 = ツール数 33 通過
- **次**: `git push` → tako 終了（Cmd-Q）→ `scripts/build-app.sh --install` → 再起動で実機確認
- 最終更新: 2026-06-15

## 残作業・既知の制約

- ホバーポップアップは読取専用（ピンは行/カードの 📌）。ポップアップへマウスを移すと行 hover が
  切れるため、操作要素はポップアップに置かない設計（VSCode 流）
- ピンは `set_pin` でカスケード配置（160+28n, 120+28n）。中身が消えた（kill 等）ピンはその
  フレームでは描かれず、次の操作で掃除される。pin_key はグループに `1<<63` を立てて衝突回避
- F2: 折りたたみは TabId キーなので閉じたタブ ID が残骸として残る → save 時に現存タブのみ永続化
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知・本変更と無関係）
- ピンの永続化（再起動またぎ）は未実装＝今回の意図的スコープ。要望あれば layout.json へ追加可

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。コミット前は必ず
  `cargo fmt --all --check`（exit code）を確認する。パイプ + `&& echo OK` は誤判定するので注意
- **ライブプレビューは追加実装不要**: `on_term_event` が全ペインの出力で `cx.notify()` を
  呼ぶので、`terminal_screen_lines` ベースのプレビューは再描画で勝手にライブ化する
- **on_hover は cx.listener と組める**（on_click と同形 `Fn(&bool,&mut Window,&mut App)`）。
  ホバー離脱クリアは「自分が対象のときだけ」に限定（新しい hover を消さない）
- **ホバーポップアップに操作要素を置かない**: ポップアップへマウスを移すと行 hover が切れる。
  ピン等の操作は行/カード側のボタンへ（VSCode 流）
- **ピンドラッグは既存マウス経路に統合**（dragging_pin を on_mouse_move/up の畳み込みに追加）
- **gpui の戻り型**: `.id()` を付けた要素は `Stateful<Div>`。ヘルパ戻り型に注意。shadow 系
  メソッドはこの rev に無い（border で代用）

## 現フェーズで Read すべき設計書

- タブツリー/プレビュー/ピン再修正時: `requirements.md` FR-2.15 / FR-2.16（特に 13〜16）
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 主な変更（F1〜F4）: `crates/tako-app/src/main.rs`（PreviewTarget / HoverPreview /
  PinnedPreview / render_tmux_view / render_hover_preview / render_pinned_previews /
  set_pin / set_tmux_collapsed）/ `crates/tako-control/src/{protocol,dispatch,mcp,layout}.rs`
  （CollapseTab / Pin / PinnedView / list の collapsed・pinned）/ `crates/tako-cli/src/main.rs`
  （`tako collapse` / `tako pin`）
