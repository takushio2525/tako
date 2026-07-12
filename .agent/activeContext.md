# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#152 完了）

#152 / PR #154 は squash merge 済み。`scripts/build-app.sh --install` で
`/Applications/tako.app` へ配置済み。

- PDF canvas を画像左上へ固定し、選択矩形を専用最前面 layer で合成
- syntect の行末改行を維持し、読み取り / 編集の構文解決を標準言語セット全体へ共通化
- TypeScript は JavaScript 文法へフォールバック
- Metal scene の RGBA 読み戻しで実ピクセル差分を検証

## 検証済み

- PDF 選択: 2,475px 変化
- C++: 読み取り 7,173px / 編集 7,277px 変化
- Python: 読み取り 7,089px / 編集 7,193px 変化
- workspace build / test（483 passed）/ fmt / clippy 全緑
- 通常隔離 selftest が `TAKO_APP_SELF_TEST_OK` まで完走
- インストール済み app / CLI は生成物と一致、codesign 検証成功

## 次の一手

- ユーザー最終確認: tako 再起動後、実マウスで PDF ドラッグ選択
- `.cpp` / `.py` と任意のコードファイルを読み取り / 編集で開き、見た目を確認
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md` FR-3.2〜FR-3.5
- 手動確認: `.agent/manual-checks.md`「PDF 選択描画・標準言語セット色分け」
