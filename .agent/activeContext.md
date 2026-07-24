# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-24・#470 紹介動画 v3 → ユーザー確認待ち）

**#470 紹介動画 v3（docs/470-promo-video-v3）**

- setup 節を作り直した: コマンド紹介（`--check` / `setup-mcp`）→
  **対話セットアップエージェント**の訴求へ。`tako setup` が質問ゼロで検出を終えると
  アシスタントが自動起動し、日本語の相談で指示ファイル・プロファイル・プロジェクト登録が決まる
- master 節に S6c を追加: **ホーム**で起動した master が projects.yaml のレジストリから
  「awesome-app」を解決し、その cwd で worker を立てる（`record-scenes.sh project`）
- v2 から維持: テロップ背景パネル・構成順（画面操作 → プレビュー → setup → master）・
  絵文字ゼロ・BGM（115 秒へ延長）・agent / preview / master / outro の素材
- 完成品: `~/Desktop/tako-promo/tako-intro-v3.mp4`（106s / 1920x1200 / H.264 + AAC）

## 次の一手

- ユーザーが v3 を視聴して setup 節の訴求・S6c の追加・尺を確認（OK なら #470 クローズ判断）
- 収録の落とし穴は台本の「収録の技術制約」7〜9 に追記済み
  （デモ HOME のキーチェーン / `tko` への `TAKO_DATA_DIR` 必須 / `--await-prompt` の中断）

## 現フェーズで Read すべき設計書

- 紹介動画: `.agent/plans/2026-07-promo-video.md`（シーン表・訴求文言・収録の技術制約）
