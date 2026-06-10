# roadmap.md — フェーズ計画

> 実装の順序と各フェーズの完了条件（Exit Criteria）。
> 「何を作るか」は `requirements.md`、「どう作るか」は `architecture.md`。

## 方針

- **最大リスク（GPUI の Windows 対応）を Phase 0 で最初に潰す**。成立しなければスタック再検討
- macOS 先行で機能を積むが、**Phase 1 以降も Windows ビルドを CI で常に通し続ける**
  （最後にまとめて移植、はしない。GPUI の Windows 成熟を継続的に追跡する意味もある）
- 各フェーズは「動くものが残る」単位で切る

## Phase 0: 技術検証スパイク（最重要）

**GPUI の Windows ビルド検証スパイク + 最小ターミナル描画 PoC**

- [ ] GPUI 単体アプリ（ウィンドウ + テキスト描画）が macOS / **Windows** 両方でビルド・起動できることを確認
- [ ] alacritty_terminal + PTY でシェルを起動し、グリッドを GPUI で描画する最小 PoC（macOS）
- [ ] 同 PoC を Windows（ConPTY）で動かす
- [ ] PTY クレート（portable-pty 等）と非同期方式（GPUI executor / tokio）を確定
- [ ] GPUI の Windows 未成熟箇所（IME・フォント・ウィンドウ管理等）を洗い出してリスト化

**Exit Criteria**: 両 OS で「シェルが動いて文字が打てる窓」が出る。
**失敗時**: GPUI を諦め、代替（iced / 自前 wgpu / Tauri 等）の再評価に戻る。この判断ごと記録する。

## Phase 1: macOS MVP（素のターミナル）

- [ ] Cargo ワークスペース構成（tako-core / tako-control / tako-app / tako-cli）確定
- [ ] タブの作成・切替・クローズ（FR-1.2）
- [ ] ペイン分割・リサイズ・フォーカス移動（FR-1.3）
- [ ] PaneTree ドメインモデルと UI の分離（GPUI 非依存の core/）
- [ ] スクロールバック・コピペ・基本的な使い心地
- [ ] Windows ビルドを CI（GitHub Actions）に組み込む

**Exit Criteria**: 日常のターミナルとして自分が常用できる（macOS）。

## Phase 2: Layer 1 — CLI と環境変数注入

- [ ] `TAKO_PANE_ID` / `TAKO_TAB_ID` / `TAKO_SOCKET` / `TAKO_TOKEN` 注入（FR-2.1.1）
- [ ] IPC サーバー（Unix domain socket + JSON-RPC）
- [ ] `tako split` / `send` / `focus` / `list`（FR-2.2.1〜2.2.4）
- [ ] `tako read` / `close` / `title`（FR-2.2.5〜2.2.6）
- [ ] 呼び出し元ペイン自動特定とアプリ外実行時のエラー（FR-2.2.7〜2.2.8）

**Exit Criteria**: シェルスクリプトから同タブ内にペインを生やしてコマンドを流し込める。
tmux 系オーケストレーター（spawn-worker.sh 等）が CLI 差し替えだけで動く。

## Phase 3: Layer 2 — 内蔵 MCP サーバー（最大の差別化点）

- [ ] MCP サーバー内蔵（IPC と操作セットを共有、FR-2.3.1）
- [ ] `TAKO_MCP_URL` による自動発見 + トークン認証（FR-2.3.2 / 2.3.4）
- [ ] 呼び出し元ペイン特定と同タブスコープ制限（FR-2.3.3）
- [ ] Claude Code をリファレンスとした設定ゼロ接続の実証
- [ ] ペインの role ラベルと状態表示 UI（FR-2.1.3〜2.1.4）

**Exit Criteria**: tako 内で Claude Code を起動し、**何も設定せずに**
「dev サーバーを隣のペインで起動して」が通る。

## Phase 4: Layer 3 — パッシブ検知

- [ ] OSC 7 / 133 シェル統合（zsh / bash / fish 同梱、FR-2.4.1）
- [ ] listen ポート検知（macOS: libproc、FR-2.4.2）
- [ ] 提案チップ UI（opt-in、強制分割なし、FR-2.4.3〜2.4.4）

**Exit Criteria**: `npm run dev` を打つと「localhost:5173 をプレビューで開く？」チップが出る。

## Phase 5: コンセプト② — ワークスペース機能

- [ ] 左サイドバー: cwd 連動ファイルツリー（FR-3.1）
- [ ] コードプレビュー + シンタックスハイライト（FR-3.2、ハイライタ選定）
- [ ] Markdown プレビュー（FR-3.3）・軽い編集（FR-3.5）
- [ ] 右サイドバー: git graph（FR-3.6）
- [ ] PDF プレビューの要否再判断（FR-3.4）

**Exit Criteria**: エージェントの成果物（コード・README）を tako から出ずに確認・微修正できる。

## Phase 6: Windows 本格対応

- [ ] ConPTY・named pipe・PowerShell シェル統合・ポート検知の Windows 実装を仕上げる
- [ ] Phase 0 で洗い出した GPUI Windows 未成熟箇所の再評価と回避策
- [ ] Windows でのフルシナリオ（Phase 3 の Exit Criteria 相当）達成

**Exit Criteria**: Windows ユーザーに「使ってみて」と言える品質。

## Phase 7: 公開準備（v0.1.0）

- [ ] バイナリ配布（GitHub Releases、macOS は notarization、Homebrew / winget は検討）
- [ ] README・スクリーンショット・デモ GIF 整備
- [ ] リポジトリ公開（private → public 化）、CONTRIBUTING / Issue テンプレート

**Exit Criteria**: 公開して他人がインストールできる。
