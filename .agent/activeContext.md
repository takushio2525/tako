# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-16・GitHub Releases 配布整備完了）

`scripts/release.sh` を新設し、ローカルビルド → zip 生成 → GitHub Release アップロードの
一発スクリプトを整備。README にダウンロード・インストール手順（Gatekeeper 対処法含む）を追加。

- **release.sh**: `build-app.sh` → `ditto -c -k` zip → `gh release create`。デフォルトは
  zip 生成まで、`--publish` で公開リリース、`--draft` でドラフト、`--skip-build` でビルド省略
- **README**: 「ダウンロード / Download」セクション新設。zip DL → 展開 → /Applications ドラッグ
  → Gatekeeper「このまま開く」の 4 ステップ
- **バージョニング**: Cargo.toml `workspace.package.version`（現在 `0.1.0`）を build-app.sh /
  release.sh が読む。zip 名は `tako-v0.1.0-macos-arm64.zip`（アーキテクチャ自動判定）
- push 済み（`8c0ce17`）。実際のリリース作成はユーザー判断待ち
- 最終更新: 2026-06-16

## 残作業・既知の制約

- ホバーポップアップは読取専用（ピンは行/カードの 📌）。ポップアップへマウスを移すと行 hover が
  切れるため、操作要素はポップアップに置かない設計（VSCode 流）
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知・本変更と無関係）
- ピンの永続化（再起動またぎ）は未実装＝意図的スコープ

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。コミット前は必ず
  `cargo fmt --all --check`（exit code）を確認する
- **リリース作成**: `scripts/release.sh --publish` で v0.1.0 を作成可能。初回リリース前に
  バージョンを上げるなら Cargo.toml の `workspace.package.version` を編集

## 現フェーズで Read すべき設計書

- タブツリー/プレビュー/ピン再修正時: `requirements.md` FR-2.15 / FR-2.16（特に 13〜16）
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- リリーススクリプト: `scripts/release.sh`
- ビルドスクリプト: `scripts/build-app.sh`
