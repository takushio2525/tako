# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-30 午後・ユーザー要望バッチ 6 件完了）

- 本日 merge・install・GUI 再起動まで完了（main = `fbad6d7`、/Applications = 0.6.1 系）:
  - #652 PC 再起動後に master ペインだけ claude resume されない問題（transcript の全 config dir
    走査 + resume コマンドへの CLAUDE_CONFIG_DIR 前置。PR #661）
  - #656 Markdown プレビュー高品質化（GFM テーブル + 配色・タイポグラフィ全面。PR #667）
  - #668 visual-test のインデントガイド節の先行破損（検査側の走査範囲を実描画行から導出。
    これで visual-test が全節完走するようになった。PR #673）
  - #666 AI コマンド提案カード `tako show-command` / MCP `tako_show_command`（130 ツール。
    コピー = 論理 1 行・新規ペイン実行。master/solo/worker prompt に提示規範入り。PR #675）
  - #669 コードプレビュー本体のライトテーマ構文色（`Theme::adapt_syntax_color` で md と構造統一。PR #677）
  - #676 `tako run` が focus 未指定でフォーカスを奪う問題（spawn_command_pane で統一。PR #678）
- 追加 merge（07-30 午後）: #680 md プレビューのリンク ⌘+クリック（http/https のみ）+
  コードブロックのコピーボタン（常時表示・アイコンのみ。`opacity(0)`+group_hover は
  実機で復帰しないため不採用）+ `tako preview-copy-code`（131 ツール）。PR #685 = `9758a6b`。
  /Applications へ install 済み（GUI 再起動待ち）
- GUI 再起動後の実機確認済み: 復元 3 タブ / 4 ペイン（tmux 再 attach、喪失ゼロ）、
  #666 カードの実描画・コピー pbpaste 一致・新規ペイン実行・フォーカス不変（スクショ確認）
- worker アカウント既定を univ（opus-5 解禁済み）へ変更（profiles: worker_account=univ /
  worker_model=claude-opus-5。accounts.yaml の univ default_model も opus-5）

## 次の一手

1. ~~ユーザー目視~~ → **07-31 に「いい感じ」と肯定評価済み**（md プレビュー・カード位置・
   コピーボタン含む今回バッチ全体）。追加フィードバックが来たら都度 Issue 化
2. #658（セルフテスト由来の worker レジストリ残留 + GC 不全）— 起票済み・未着手
3. #652 の真の実地検証は次回 PC 再起動時（persist.log の Claude resume 数と master 復元を確認）
4. 今夜の nightly が本日 6 件を 0.6.2 として自動リリース見込み
5. **#691 GUI モード: G2（#702）まで完了**（08-01）: claude ペインのチャットビュー（会話 md 表示・
   ctx バー・busy 表示・worker read-only）。G1 の alt_screen 誤判定（persist 環境で GUI モード
   無反応）も同梱修正。次は G3（入力・承認・スラッシュボタン + カードのインライン埋め込み）
6. 08-01 完了分: #703 カード専用帯（会話と重なりゼロ）/ #708 Finder「このアプリケーションで開く」
   （21 拡張子・既定アプリ不変）。全 merge・install・GUI 再起動・実機確認済み
7. 08-02 完了分: #725 チャットビューの選択・コピー（発話をまたぐドラッグ選択 + ⌘C / ⌘A、
   発話単位のコピーボタン、`tako chat copy` / MCP `tako_chat_copy` = 133 ツール）
8. 08-02 完了分: #738 設定画面プロファイルタブの描画崩壊（`flex_wrap` の幅未確定でチップが
   1 個ずつ縦に落ち次の行へ重なる問題を `row_wrapping` で根治。visual-test に重なり検査を常設）

## 未着手・持ち越し

- cask caveats 文面の是正 / #601 案 2（FR-2.14.5）/ #632 / #633 / #638 / #651
- キュー: #519 ⑤⑥ / #513 Windows 実機配線 / #542 / #541 Phase 2 / Windows #467→#517 / #434 宣伝

## GUI 検証の環境知見（次回の時間浪費を防ぐ）

- **蓋閉じ（外部ディスプレイ無し）では GUI 検証不可**。`ioreg -r -k AppleClamshellState` を最初に確認
- **画面ロック / スクリーンセーバー中も不可**: `screencapture` が黒画かロック画面になる
- **`cargo test` は起動用バイナリを更新しない**: GUI 実測の前に `cargo build` が要る
- **`open` は呼び出し元シェルの env を継承する**: master ペインからの再起動は
  `env -u CLAUDE_CONFIG_DIR open -a tako` が必須
- **隔離インスタンスへの CLI 接続**: `TAKO_ISOLATED=1` の discovery は
  `$TMPDIR/tako-iso-discovery-<pid>/control.json`。socket / token を `TAKO_SOCKET` /
  `TAKO_TOKEN` に渡すと本番に触れず dispatch を叩ける
- **マウス操作は visual-test から合成できる**（#725）: `window.dispatch_event(
  gpui::PlatformInput::MouseDown/MouseMove/MouseUp)` で GPUI のヒットテストとリスナー配線まで
  通せるので、画面ロック中でもドラッグ選択・クリックを機械検証できる。**`AnyWindowHandle::update`
  から呼ぶ**こと（`WindowHandle<V>::update` の中だとルートビューの二重借用で panic する）
- **`git stash` はリポジトリ共有**（worktree でも）。退避は `git diff > patch` を使う
- master の Bash には `TAKO_PANE_ID` が入っていないことがある → tako CLI は `--pane` 明示

## 現フェーズで Read すべき設計書

- GUI モード（#691 G2 以降）に手を入れる: `.agent/plans/2026-07-gui-mode.md`（正）+
  `crates/tako-core/src/ui_mode.rs`（判定表）+ `crates/tako-app/src/starter.rs`
- **Markdown の描画に手を入れる**: `crates/tako-app/src/md_view.rs`（幾何とテーマ色の唯一の
  実装。プレビューペインとアップデート詳細画面が共有。#690）+ `.agent/requirements.md` FR-3.3.1
- Markdown プレビュー固有（選択・検索・コピーボタン）: `crates/tako-app/src/preview_render.rs`
- コマンド提案カードに手を入れる: `crates/tako-core/src/command_card.rs` + dispatch の ShowCommand 系
- 復元・resume 系: `crates/tako-control/src/transcript.rs` + `sessions.rs` + `.agent/requirements.md` FR-5.9/FR-5.12
