# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-15・サイドバー tmux ビュー一本化 = 3 要望完了）

commit `f64d2a3` で統合 tmux ビュー + 退避を 3 点改修（全 QA 緑、未プッシュ → push 予定）。

- **二重化解消（FR-2.16.9 統合）**: attach 中の外部 tmux セッションを独立ブロックで重複
  表示せず、**ホストペイン行の下へインデント入れ子表示**（`render_attached_session_rows`）。
  ホスト行 detail にセッション名を出し「1 セッション = タブツリー上 1 箇所」に統合。
  データ層 `tmux_view_groups().sessions` は不変（self-test 61f 維持）
- **表示分類（FR-2.16.12）**: 各ペイン行に表示中（アクティブタブ）/ バックグラウンドの
  バッジ（`surface_badge`）。list の各ペインに `surface`（foreground/background）公開
- **退避のタブ別分離（FR-2.15.6）**: `Workspace::shelved` を `Vec<ShelvedPane>`（由来タブ
  ID + タブ名スナップショット）へ。タブツリーは各タブ枠内に由来退避をバックグラウンド行
  表示、閉じたタブ由来は「タブ \<名前\>（閉じたタブ）」へ集約。ドロワーも由来タブごとに
  グループ化。復帰は既定で由来タブへ。`ShelvedList` に origin_tab/origin_tab_title/surface、
  `unshelve` は target 省略で由来タブ復帰（開発不変条件）
- **検証済み**: build / clippy(-D warnings) / fmt / test 全緑（app33 / cli10 / control54 /
  core103）。セルフテストは項目70 PDF（既知 Core Graphics）以外緑 = 1〜69 通過
- **次**: `git push` → tako 終了 → `scripts/build-app.sh --install` → 再起動で実機確認
- 最終更新: 2026-06-15

## 残作業・既知の制約

- ドロワーのグループは横並び（タブ見出し + カード行）。グループ見出し分 16px を本文高さから
  減算（`DRAWER_GROUP_HEADER`）。サムネイル resize は冪等で近似
- 閉じたタブ由来の退避は ID が後続新規タブに再利用されないよう `TabId::from_raw` で採番予約
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知・本変更と無関係）
- ドロワーは現状リサイズ不可（既定 240px 固定）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。コミット前は必ず
  `cargo fmt --all --check`（exit code）を確認する。パイプ + `&& echo OK` は誤判定するので注意
- **ShelvedPane の pass-through**: `id()/title()/role()` を備えるため既存呼び出しは無改修で
  通る。pane 本体は `.pane()`、由来は `.origin_tab()/.origin_tab_title()`
- **退避は最後のペイン/タブで由来タブが閉じる**: その場合も由来タブ名はスナップショット済み
  なので「閉じたタブ」グループで親を明記できる（空タブは残さない方針 = ユーザー承認済み）
- **大きい置換は Edit より Bash + python3 が安全**（描画ループ削除は python で実施）
- **gpui の戻り型**: `.id()` を付けた要素は `Stateful<Div>`。ヘルパ戻り型に注意

## 現フェーズで Read すべき設計書

- 退避/タブツリー再修正時: `requirements.md` FR-2.15 / FR-2.16
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 主な変更: `crates/tako-core/src/workspace.rs`（ShelvedPane）/
  `crates/tako-app/src/main.rs`（render_tmux_view / render_drawer）/
  `crates/tako-control/src/{dispatch,layout}.rs`
