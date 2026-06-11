# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> 直近の作業のみ参照、エントリ 30 件超 or 14 日より古いものは `progress-archive.md` への移送を提案する。
> 自動削除はしない。常にユーザー確認を経る。

## 追記フォーマット

```markdown
## YYYY-MM-DD
- {一行サマリ。何を/どこを/結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-06-11（プロジェクト開始）

- リポジトリ初期化（git init + GitHub private リポ作成）、AGENTS.md / .agent/ 構成導入
- 仕様書一式作成: concept / requirements / architecture / roadmap + README（日英）+ LICENSE（Apache-2.0）
- 未決事項: MCP トランスポート（Phase 3）、ハイライタ選定（Phase 5）、`tako` コマンド名衝突調査、Linux 対応の扱い
- 次: Phase 0 — GPUI Windows ビルド検証スパイク + 最小ターミナル描画 PoC

## 2026-06-11（Phase 0 完了）

- Phase 0 実施: GPUI 最小ウィンドウ（crates.io 0.2.2 / git rev 固定の両方）+ alacritty_terminal 最小ターミナル PoC が macOS で成功。スタック採用確定、GPUI は git rev 固定戦略
- Windows は Web 調査で成立見込み高と判断（Zed 正式リリース済み）。実ビルドは Phase 1 CI / Phase 6 実機へ残タスク化。検証結果・ハマりどころは architecture.md / poc/README.md に反映
- 関連コミット: `c1427b4` `f0e68ff` + ドキュメント反映コミット
- 次: Phase 1 — Cargo ワークスペース構成と CI（windows スモーク含む）から着手

## 2026-06-11（Phase 1 前半完了 + 仕様拡充）

- 仕様: FR-2.5 AI レイアウト操作セット / 設計原則 5 AI フルコントロール / FR-2.6 注釈レイヤ / FR-3.8 Web ビューペイン（方式候補は architecture.md）を要件化
- 実装: 4 クレートワークスペース + PaneTree ドメインモデル（GPUI 非依存・テスト 24 本）+ tako-app 最小ターミナル（セルフテスト緑）+ CI が macOS / Windows 両緑（Phase 0 残タスクの Windows スモーク完了）
- 関連コミット: `c1ae3e0` `bd69d91` `5f26d45` `559bbc5` `d9c5f8b` `fc3dad2`
- 次: Phase 1 後半 — 複数ペイン描画・タブ UI・スクロールバック
