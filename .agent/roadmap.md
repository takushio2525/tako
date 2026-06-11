# roadmap.md — フェーズ計画

> 実装の順序と各フェーズの完了条件（Exit Criteria）。
> 「何を作るか」は `requirements.md`、「どう作るか」は `architecture.md`。

## 方針

- **最大リスク（GPUI の Windows 対応）を Phase 0 で最初に潰す**。成立しなければスタック再検討
- macOS 先行で機能を積むが、**Phase 1 以降も Windows ビルドを CI で常に通し続ける**
  （最後にまとめて移植、はしない。GPUI の Windows 成熟を継続的に追跡する意味もある）
- 各フェーズは「動くものが残る」単位で切る

## Phase 0: 技術検証スパイク（最重要）→ ✅ 完了（2026-06-11、条件付き）

**GPUI の Windows ビルド検証スパイク + 最小ターミナル描画 PoC**

- [x] GPUI 単体アプリ（ウィンドウ + テキスト描画）が macOS でビルド・起動（crates.io 版 / git 版両方で確認）
- [x] Windows は**調査ベースで成立見込み高と判断**（Zed Windows 正式リリース済み・単体利用実績あり。
      実機が無いため実ビルドは未実施 → 残タスクとして下記に移管）
- [x] alacritty_terminal + PTY でシェルを起動し、グリッドを GPUI で描画する最小 PoC（macOS、`poc/03-term-poc`）
- [x] **（残タスク → Phase 1 で完了、2026-06-11）** Windows でのビルド・スモーク:
      GitHub Actions windows ランナーで本実装ワークスペース（gpui git 版 + alacritty_terminal 含む）の
      build + test が成功（PoC 相当を上回る検証。Spectre-mitigated libs は CI で追加インストール）
- [ ] **（残タスク → Phase 6 へ）** Windows 実機での動作検証（ConPTY・IME・フォント描画）
- [x] PTY クレート確定: **alacritty_terminal::tty**（portable-pty 不要）。非同期: **GPUI executor + futures channel**（tokio 不要）
- [x] GPUI の Windows 未成熟箇所をリスト化（`architecture.md` の「Phase 0 検証結果」節）

**判定**: macOS では Exit Criteria（シェルが動いて文字が打てる窓）達成。Windows は実機が無く
調査ベースの判断だが、Zed 本体の正式リリース実績から**スタック採用を確定**。
GPUI バージョン戦略は **zed リポ git rev 固定**（`architecture.md` 参照）。

## Phase 1: macOS MVP（素のターミナル）→ 前半完了（2026-06-11）

- [x] Cargo ワークスペース構成（tako-core / tako-control / tako-app / tako-cli）確定
- [x] PaneTree ドメインモデルと UI の分離（GPUI 非依存の core/。分割・削除・フォーカス・リサイズ・
      均等化・layout 取得、テスト 24 本。操作 API は FR-2.5 と 1:1 対応前提）
- [x] tako-app がワークスペース構成上で最小ターミナル（1 ペイン）を起動
      （`TAKO_SELF_TEST=1` で入力 → PTY → グリッド反映を機械検証可能）
- [x] Windows ビルドを CI（GitHub Actions）に組み込む（macOS / Windows 両ランナーで build + test 緑）
- [ ] タブの作成・切替・クローズ UI（FR-1.2。ドメインモデルは実装済み、描画と操作が未）
- [ ] ペイン分割・リサイズ・フォーカス移動 UI（FR-1.3。同上）
- [ ] スクロールバック・コピペ・基本的な使い心地（色・カーソル描画・PTY リサイズ含む）

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
- [ ] FR-2.5 レイアウト操作セットの拡充（リサイズ・レイアウトプリセット・タブ操作 FR-2.5.10。
      ファイル/URL 系 FR-2.5.11〜12 は Phase 5 のペイン種別実装後）

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
- [ ] Web ビューペイン実現方式の検証スパイク（FR-3.8。WKWebView / WebView2 重ね合わせ。
      候補とリスクは `architecture.md`「Web ビューペイン」節。暫定は外部ブラウザ起動でも可）
- [ ] AI 誘導・注釈オーバーレイ（FR-2.6）と `tako_open_file` / `tako_open_url` / `tako_annotate`
      （FR-2.5.11〜12。設計原則 5「AI フルコントロール」）

**Exit Criteria**: エージェントの成果物（コード・README）を tako から出ずに確認・微修正できる。
「あのファイル開いて見せて」「ここを見て」が AI 経由で通る。

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
