# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-12・#126 コードプレビュー編集）

FR-3.5 コードプレビューの軽量編集を実装。テキスト／コードファイルをその場で編集し、
dirty 表示と ⌘S 保存、外部変更競合の拒否に対応する。

- core: UTF-8 安全な `TextBuffer`（入力・削除・改行・選択・カーソル移動・保存）
- control: `PreviewEdit` / `PreviewApply` / `PreviewSave`
- CLI: `tako edit start|status|apply|save|stop`
- MCP: `tako_preview_edit` / `tako_preview_apply` / `tako_preview_save`（計 56 ツール）
- UI: 編集切替、キャレット、dirty「●」、保存ボタン、IME 振り分け
- 安全制限: 非テキスト・非 UTF-8・バイナリ・末尾省略ファイルは編集不可

## 直近の観点

- 保存時は編集開始時の元バイト列と現ファイルを比較し、外部変更なら上書きしない
- Unix は一時ファイル + rename、Windows は比較後の truncate + sync（原子性差を仕様化）
- PDF #124 の `preview_line_bounds` / `preview_line_texts` と選択分岐は維持
- GUI の実 IME・マウス操作は実 .app で手動確認する
- CI は停止中。ローカル build / test / fmt / clippy を品質ゲートにする

## 次の一手

- FR-3.5 の実機常用フィードバックを反映
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- プレビュー編集: `.agent/requirements.md` FR-3.5、`.agent/architecture.md`「コンセプト②の実現」
- UI 手動確認: `.agent/manual-checks.md`「コードプレビュー軽量編集」
