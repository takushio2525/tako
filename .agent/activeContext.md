# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#145 プレビュー選択座標 / PDF / 編集色）

#145 完了・PR #151 squash merge 済み・`build-app.sh --install` 済み。
#150 の 3 クラッシュは selftest 66b-2 の二重 update と確認して調査コメント後 close 済み。

- Code / Markdown は GPUI `TextLayout` の実 shaping 座標と最近傍 UTF-8 キャレットで hit-test
- PDF は PDFKit の行 / 文字矩形を表示座標へ変換して選択・ハイライト・コピー
- 編集モードでも syntect 色を維持し、選択 / キャレットを合成
- selftest 40 は固定待ちを廃止し、実 CLI の IPC 応答完了後に根分割状態を検証

## 検証済み

- workspace build / test / fmt / clippy（`-D warnings`）全緑
- `TAKO_SELF_TEST=1 cargo run -p tako-app` が `TAKO_APP_SELF_TEST_OK` まで完走
- `/Applications/tako.app` と生成物の実行バイナリ SHA-256 一致、codesign 検証成功

## 次の一手

- tako 再起動後、`.agent/manual-checks.md` #145 節の実マウス選択を GUI で確認
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md` FR-3.2〜FR-3.5
- 手動確認: `.agent/manual-checks.md`「プレビュー選択座標・編集時色分け」
