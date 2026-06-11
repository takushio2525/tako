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

## Phase 1: macOS MVP（素のターミナル）→ ✅ 実装完了（2026-06-11。常用判断はユーザー確認待ち）

- [x] Cargo ワークスペース構成（tako-core / tako-control / tako-app / tako-cli）確定
- [x] PaneTree ドメインモデルと UI の分離（GPUI 非依存の core/。分割・削除・フォーカス・リサイズ・
      均等化・layout 取得、テスト 24 本。操作 API は FR-2.5 と 1:1 対応前提）
- [x] tako-app がワークスペース構成上で最小ターミナル（1 ペイン）を起動
      （`TAKO_SELF_TEST=1` で入力 → PTY → グリッド反映を機械検証可能）
- [x] Windows ビルドを CI（GitHub Actions）に組み込む（macOS / Windows 両ランナーで build + test 緑）
- [x] タブの作成・切替・クローズ UI（FR-1.2。タブバー + cmd+T / cmd+W / cmd+数字 / cmd+shift+[]）
- [x] ペイン分割・リサイズ・フォーカス移動 UI（FR-1.3。cmd+D / cmd+shift+D / cmd+alt+矢印 /
      ctrl+cmd+矢印。iTerm2 踏襲）。境界線のマウスドラッグリサイズも実装済み
      （tako-core `borders`/`set_split_ratio`/`ratio_for_position`、UI は透明ハンドル + cursor）
- [x] スクロールバック・コピペ・基本的な使い心地（256 色 / truecolor・カーソル・選択コピー
      （copy-on-select）・ブラケットペースト・PTY リサイズ追従・exit でペイン自動クローズ。
      描画色はすべて tako-core の Theme 経由（FR-4 の実装指針））

**Exit Criteria**: 日常のターミナルとして自分が常用できる（macOS）。
→ 実装・機械検証（セルフテスト 13 項目）は完了。常用フィードバックは使いながら Phase 2 以降で拾う。

## Phase 2: Layer 1 — CLI と環境変数注入 → ✅ 完了（2026-06-11）

- [x] `TAKO_PANE_ID` / `TAKO_TAB_ID` / `TAKO_SOCKET` / `TAKO_TOKEN` 注入（FR-2.1.1。
      `TAKO_MCP_URL` は Phase 3 の MCP 実装時に注入開始）
- [x] IPC サーバー（Unix domain socket + JSON-RPC + トークン認証。操作ディスパッチは
      `tako-control::dispatch` に一元化し Phase 3 の MCP と共有する。
      Windows named pipe は Phase 6 の TODO → `architecture.md`「IPC トランスポート」節）
- [x] `tako split` / `send` / `focus` / `list`（FR-2.2.1〜2.2.4）
- [x] `tako read` / `close` / `title`（FR-2.2.5〜2.2.6）。加えて FR-2.5 から
      `resize` / `equalize` / `tab new・select・move-pane`（FR-2.5.6〜7 / 2.5.10）を前倒し実装
- [x] 呼び出し元ペイン自動特定とアプリ外実行時のエラー（FR-2.2.7〜2.2.8）

**Exit Criteria**: シェルスクリプトから同タブ内にペインを生やしてコマンドを流し込める。
→ セルフテスト 29 項目（ペイン内シェルから実 `tako` CLI を叩く e2e 含む）で機械検証済み。
tmux 系オーケストレーターの実地差し替えは Phase 3 以降の常用で確認する。

## Phase 3: Layer 2 — 内蔵 MCP サーバー（最大の差別化点）→ ✅ 完了（2026-06-11）

- [x] MCP サーバー内蔵（IPC と操作セットを共有、FR-2.3.1。エンジン + Streamable HTTP +
      stdio ブリッジ `tako mcp serve` の構成は `architecture.md`「Layer 2」節）
- [x] `TAKO_MCP_URL` による自動発見 + トークン認証（FR-2.3.2 / 2.3.4。Bearer + Origin 検証。
      Claude Code は環境変数からの自動発見機構を持たないため、現実解は
      user スコープへの stdio ブリッジ登録 1 回 → 以後ゼロ設定）
- [x] 呼び出し元ペイン特定と同タブスコープ制限（FR-2.3.3。特定 = TAKO_PANE_ID /
      X-Tako-Pane、省略時デフォルトが同タブに解決。ハード強制は FR-2.3.5 と併せて後段）
- [x] Claude Code をリファレンスとした設定ゼロ接続の実証
      （`scripts/verify-claude-mcp.sh`。stdio / HTTP 両経路で実 `claude -p` が通る）
- [x] ペインの role ラベルと状態表示 UI（FR-2.1.3〜2.1.4。右上バッジ + 状態ドット。
      状態は OSC 133 由来、タブ集約は CommandState::aggregate。2026-06-11 完了）
- [x] FR-2.5 レイアウト操作セットの MCP 公開（12 ツール。
      ファイル/URL 系 FR-2.5.11〜12 は Phase 5 のペイン種別実装後）

**Exit Criteria**: tako 内で Claude Code を起動し、**何も設定せずに**
「dev サーバーを隣のペインで起動して」が通る。
→ 機械検証（セルフテスト 36 項目 + verify-claude-mcp.sh）で経路は実証済み。
GUI 内での常用体験は初回登録（`claude mcp add --scope user`）後に日常使いで確認する。

## Phase 3.5: 日常使い品質 → ✅ 実装完了（2026-06-11。常用での手動確認はユーザー）

> Phase 3 完了を機にユーザーが tako を日常ターミナルとして使い始める。
> そのためのブロッカー除去タスク群。

- [x] IME 変換中表示（FR-1.9 = Must）: GPUI `EntityInputHandler` で未確定文字列の
      インライン表示（細下線）+ 注目文節の強調（太下線 + 選択色）+ 候補ウィンドウの
      位置出し（`bounds_for_range`）。機械検証はセルフテスト 37〜39、
      見た目・実 IME の確認は `.agent/manual-checks.md` の手動チェックリスト
- [x] .app バンドル化: `scripts/build-app.sh`（icns 生成・Info.plist・ad-hoc 署名・
      tako CLI 同梱・`--verify` でバンドル版セルフテスト・`--install` で /Applications 配置）。
      release profile（thin LTO + strip）新設。アイコンは A 案採用（`assets/icon/README.md`）

**Exit Criteria**: Dock から起動した tako で、日本語入力を含む日常作業を常用できる。
→ 機械検証は完了。実 IME の見た目（manual-checks.md）と常用フィードバックはユーザーが
日常使いで確認する。配布署名 / notarization は Phase 7。

## Phase 4: Layer 3 — パッシブ検知 → 前半完了（2026-06-11。OSC 統合 + 状態公開）

- [x] OSC 7 / 133 シェル統合（zsh / bash / fish 同梱・自動注入、FR-2.4.1。
      検知は PTY タップ（tako-core::osc_tap）、cwd / state / exit_code は list・MCP に公開、
      split は分割元 cwd を継承。zsh はセルフテスト 41/41b で e2e 済み、bash / fish は
      manual-checks.md で手動確認）
- [x] listen ポート検知（macOS: libproc、FR-2.4.2。2026-06-12 完成。tty 突き合わせで
      ペイン配下を判定し、list / MCP に `listen_ports` を公開。詳細は `architecture.md`
      「Layer 3」節。Windows は Phase 6）
- [ ] 提案チップ UI（opt-in、強制分割なし、FR-2.4.3〜2.4.4。**着手前に表示位置・
      承諾アクション（Web ビュー未実装のため暫定は外部ブラウザ open）をユーザーへ確認**）
- [ ] 待ちエージェント集約センター: 全タブの入力待ち / 完了 / 質問ありを集約表示し
      クリックでジャンプ（FR-2.10。エージェント監視系のタブ横断版）
- [x] タブ・ペインの AI 自動リネーム（FR-2.12。**方式 1 = tako 常駐**で 2026-06-12 完成。
      検知ループ + デバウンス + `claude -p`（haiku）+ ヒューリスティックフォールバック、
      手動優先（TitleSource）、`tako tab rename` / `tako autorename` + MCP 2 ツール（計 17）。
      実装詳細は `requirements.md` FR-2.12 実装メモ）
- [x] tmux セッションの見える化タブ tmuxview（FR-2.13。右端固定タブ + 一覧 + 確認つき
      kill。tako-core::tmux（取得層）と表示を分離、`tako tmux list/kill` + MCP 2 ツール +
      tty 突き合わせの対応付け。2026-06-12 要望・同日完成。見た目は manual-checks で常用確認）

**Exit Criteria**: `npm run dev` を打つと「localhost:5173 をプレビューで開く？」チップが出る。

## Phase 5: コンセプト② — ワークスペース機能

- [ ] 左サイドバー: cwd 連動ファイルツリー（FR-3.1）
- [ ] コードプレビュー + シンタックスハイライト（FR-3.2、ハイライタ選定）
- [ ] Markdown プレビュー（FR-3.3）・軽い編集（FR-3.5）
- [ ] 画像プレビューペイン（FR-3.10。PNG / JPEG / SVG / GIF / WebP。`show_file` 系の
      画像対応を含む。FR-2.7.6 の複数案並列比較は画像ペインを並べて実現する）
- [ ] 右サイドバー: git graph（FR-3.6）
- [ ] PDF プレビューの要否再判断（FR-3.4）
- [ ] Web ビューペイン実現方式の検証スパイク（FR-3.8。WKWebView / WebView2 重ね合わせ。
      候補とリスクは `architecture.md`「Web ビューペイン」節。暫定は外部ブラウザ起動でも可）
- [ ] AI 誘導・注釈オーバーレイ（FR-2.6）と `tako_open_file` / `tako_open_url` / `tako_annotate`
      （FR-2.5.11〜12。設計原則 5「AI フルコントロール」）
- [ ] diff ビューアペイン（FR-3.9）と AI 成果物プレゼンテーション `show_file` / `show_diff` /
      `show_url`（FR-2.7。ツール説明文への「タスク完了時は成果物を提示せよ」規範埋め込み含む）
- [ ] ワンクリックフィードバック: 提示された diff・プレビューへの範囲選択コメント /
      「OK」ボタン → MCP 経由でエージェント入力へ（FR-2.8。会話ループの双方向化）
- [ ] どこでも AI 呼び出し cmd+K（ペイン内容・選択テキスト・cwd を文脈として自動添付、FR-2.9）

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

## v0.2 以降（公開後の後段フェーズ）

- [ ] AI 活動タイムライン: エージェントの実行コマンド・変更ファイル・コミットを
      時系列一覧するペイン（FR-2.11。監査可能性・信頼の土台）
- [ ] セッション永続性: タブ / ペイン構成・cwd・タイトル・role の保存と復元（FR-5.1。
      長寿命エージェントセッションを tako へ移行するための前提。シェル内容の完全復元はしない）
