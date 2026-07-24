# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-24・#470 紹介動画 v2 → ユーザー確認待ち）

**#470 紹介動画 v2（docs/470-promo-video-v2）**

- テロップに半透明の暗色パネル（不透明度 0.84・角丸・テキスト幅追従）を敷いて可読性を確保
- 本編を 4 本柱に再構成: 画面操作 → プレビュー → **setup** → **master**
  （restore / remote は本編から外した。素材は残してある）
- setup 節（新規収録）= `tako setup --check` → `tako setup-mcp` → `claude mcp list` で
  `tako … ✔ Connected`。デモ HOME / デモ PATH で撮り、画面のパスは /private/tmp 配下だけ
- master 節（新規収録）= 実 `tako master` が worker 3→4 体を spawn し、同じタブに
  グリッド配置 + 右パネル orch ビュー
- 完成品: `~/Desktop/tako-promo/tako-intro-v2.mp4`（84s / 1920x1200 / BGM 付き）

## 次の一手

- ユーザーが v2 を視聴して構成・テロップ文言・尺を確認（OK なら #470 クローズ判断）
- 収録の落とし穴（隠れたウィンドウで描画が止まる）は `promo_verify` の
  「異なるフレーム数」チェックで自動検出するようにした

## 現フェーズで Read すべき設計書

- 紹介動画: `.agent/plans/2026-07-promo-video.md`（シーン表・訴求文言・収録の技術制約）
