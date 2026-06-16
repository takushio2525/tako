# Progress Archive

> `progress.md` から移送された古いエントリ。参照用。

---

## 2026-06-11（プロジェクト開始）
- リポジトリ初期化 + AGENTS.md / .agent/ 構成導入 + 仕様書一式作成

## 2026-06-11（Phase 0 完了）
- GPUI + alacritty_terminal PoC macOS 成功。スタック採用確定

## 2026-06-11（Phase 1 前半完了 + 仕様拡充）
- 4 クレートワークスペース + PaneTree モデル + CI macOS/Windows 両緑

## 2026-06-11（Phase 1 後半完了 + ビジョン・要件拡充）
- Theme / screen / TerminalSession 拡張 + タブ・ペイン分割 UI

## 2026-06-11（Phase 2 完了）
- Layer 1: TAKO_* env 注入 + IPC + tako CLI（split/send/focus/list 等）

## 2026-06-11（Phase 3 コア完了）
- Layer 2: MCP エンジン + Streamable HTTP + stdio ブリッジ

## 2026-06-11（Phase 3.5 完了）
- IME 変換中表示 + .app バンドル化

## 2026-06-11（常用初日バグ修正 + 境界ドラッグリサイズ）
- TERM/cwd 修正 + ペイン境界ドラッグリサイズ

## 2026-06-11（常用クラッシュ根治 + Phase 3 完了 + Phase 4 前半）
- login ラッパ→$SHELL 直接 spawn + role/title UI + OSC 7/133 検知 + シェル統合

## 2026-06-12（接続情報の永続化 FR-2.2.9）
- control.json 永続化 + CLI env→ファイル解決

## 2026-06-12（常用フィードバック一括対応）
- スクロールバック出し分け + Shift+Enter + IME 候補位置 + 全角選択座標

## 2026-06-12（tmuxview FR-2.13 完成）
- tmux 見える化: tako-core::tmux + TmuxList/TmuxKill + 右端固定タブ

## 2026-06-12（AI 自動リネーム FR-2.12 完成）
- TitleSource + claude -p (haiku) + ヒューリスティック + tako autorename

## 2026-06-12（listen ポート検知 FR-2.4.2 完成）
- libproc + tty 突き合わせ + listen_ports 公開

## 2026-06-12（提案チップ FR-2.4.3〜4 完成 + FR-2.14 要件化）
- 検知ペイン下端チップ + open_preview + tako portdetect

## 2026-06-12（集約センター FR-2.10 完成 = Phase 4 完了）
- agents 固定タブ + 注目度順集約 + ジャンプ。Phase 4 完了

## 2026-06-12（ファイルツリー FR-3.1/3.7 完成 → Phase 5 一時中断）
- ファイルツリー完成 → Phase 5.5 先行のため中断

## 2026-06-12（Phase 5.5 tmux バックエンド永続化 完成）
- spawn を tmux 経由に + layout.json 復元 + tmux 不在劣化

## 2026-06-12（実機リグレッション一括修正 + 情報パネル化）
- tmux_bin PATH 修正 + マウス・キー保証 e2e + 右サイドバーパネル化

## 2026-06-12（P0: CJK 全滅 + バグ (8) 接続競合 + 復元失敗の解明）
- LC_CTYPE=UTF-8 注入 + discovery instances/ 構成 + 復元バグ解明

## 2026-06-12（ウィンドウジオメトリ復元 + 引き継ぎ）
- OS ウィンドウフレーム layout.json 永続化

## 2026-06-12（スクロール・キー実機バグ一括 + スクロール制御の方式転換）
- tako-core::scroll 新設 + copy-mode 行数駆動 + コアレッシング

## 2026-06-12（要件一括登録: 配布 / セットアップ / FR-2.18 / FR-2.19 / パネル UI 刷新仕様）
- 要件登録のみ（実装なし）

## 2026-06-12（緊急修正: スクロール全滅の根治 = tmux ロケールサニタイズ + 署名安定化）
- tmux::tmux_command() ロケール注入集約 + Apple Development 署名

## 2026-06-12（パネル UI 刷新 FR-2.16.4〜2.16.8 完成）
- ステータスバー + 統合 tmux ビュー + 管理外/kill漏れ? 区別

## 2026-06-12（Esc「27u」挿入バグ根治）
- CsiUMode 導入で Esc 単押しを素の \e に

## 2026-06-13（実機バグ 3 件一括修正: 管理外誤判定 / kill 確認見切れ / ステータスバー消失）
- attach 外部セッション tty 突き合わせ + kill 確認 UI 共通化 + flex min-content 修正

## 2026-06-13（Phase 5 再開: コードプレビュー / Markdown / タブ = ワークスペース）
- FR-3.2 syntect + FR-3.3 pulldown-cmark + FR-3.1 マルチルート刷新

## 2026-06-13（D&D 3 件: tmux 取り込み / ファイルプレビュー / ペイン移動）
- TmuxOpen D&D + ファイルツリー D&D + タイトルバー D&D ペイン移動

## 2026-06-13（パフォーマンスバグ修正: UI スレッド非ブロック化 3 件）
- preview 2 段階化 + sync_filetree_roots stat 除去 + FileTree refresh 非同期化
