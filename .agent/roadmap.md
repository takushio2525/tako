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
- [x] .app バンドル化: `scripts/build-app.sh`（icns 生成・Info.plist・署名・
      tako CLI 同梱・`--verify` でバンドル版セルフテスト・`--install` で /Applications 配置）。
      release profile（thin LTO + strip）新設。アイコンは A 案採用（`assets/icon/README.md`）。
      署名はキーチェーンの Apple Development 証明書を自動検出して使う（2026-06-12 変更。
      ad-hoc はビルドごとに CDHash が変わり TCC の権限承認が毎回リセットされるため。
      無ければ ad-hoc に劣化 + 警告。`TAKO_CODESIGN_IDENTITY` で明示指定可）

**Exit Criteria**: Dock から起動した tako で、日本語入力を含む日常作業を常用できる。
→ 機械検証は完了。実 IME の見た目（manual-checks.md）と常用フィードバックはユーザーが
日常使いで確認する。配布署名 / notarization は Phase 7。

## Phase 4: Layer 3 — パッシブ検知 → ✅ 完了（2026-06-12）

- [x] OSC 7 / 133 シェル統合（zsh / bash / fish 同梱・自動注入、FR-2.4.1。
      検知は PTY タップ（tako-core::osc_tap）、cwd / state / exit_code は list・MCP に公開、
      split は分割元 cwd を継承。zsh はセルフテスト 41/41b で e2e 済み、bash / fish は
      manual-checks.md で手動確認）
- [x] listen ポート検知（macOS: libproc、FR-2.4.2。2026-06-12 完成。tty 突き合わせで
      ペイン配下を判定し、list / MCP に `listen_ports` を公開。詳細は `architecture.md`
      「Layer 3」節。Windows は Phase 6）
- [x] 提案チップ UI（FR-2.4.3〜2.4.4。2026-06-12 完成。検知ペイン下端のインラインチップ、
      承諾 = `open_preview`（当面は外部ブラウザ。Phase 5 で Web ビューペインへ差し替える
      抽象点）、OFF は settings + `tako portdetect` / MCP（計 18 ツール）。
      表示位置・承諾アクションはユーザー承認済み）
- [x] 待ちエージェント集約センター: 全タブの入力待ち / 完了 / 質問ありを集約表示し
      クリックでジャンプ（FR-2.10。2026-06-12 完成。右端固定タブ「agents」+ 全ペイン
      集約ドット、注目度順（エラー > 入力待ち > 実行中）、ジャンプは dispatch Focus 経由。
      「質問あり」は OSC 133 では区別できないため入力待ちに含めて表示）
- [x] タブ・ペインの AI 自動リネーム（FR-2.12。**方式 1 = tako 常駐**で 2026-06-12 完成。
      検知ループ + デバウンス + `claude -p`（haiku）+ ヒューリスティックフォールバック、
      手動優先（TitleSource）、`tako tab rename` / `tako autorename` + MCP 2 ツール（計 17）。
      実装詳細は `requirements.md` FR-2.12 実装メモ）
- [x] tmux セッションの見える化タブ tmuxview（FR-2.13。右端固定タブ + 一覧 + 確認つき
      kill。tako-core::tmux（取得層）と表示を分離、`tako tmux list/kill` + MCP 2 ツール +
      tty 突き合わせの対応付け。2026-06-12 要望・同日完成。見た目は manual-checks で常用確認）

**Exit Criteria**: `npm run dev` を打つと「localhost:5173 をプレビューで開く？」チップが出る。
→ 機械検証（セルフテスト 83 項目。nc -l でのチップ生成・却下・OFF トグル）は 2026-06-12 達成。
見た目と実 dev サーバーでの体験は manual-checks.md で常用確認。残は集約センター（FR-2.10）。

## Phase 5: コンセプト② — ワークスペース機能 → ⏸ 一時中断（2026-06-12。Phase 5.5 を先行）

> ユーザー指示により **Phase 5.5（tmux バックエンド永続化）を先に実装する**。
> 技術選定（syntect / git CLI / pulldown-cmark）は確定済み（`architecture.md`）。
> 再開時はコードプレビュー（FR-3.2）+ `tako_open_file` から
> （ファイルツリーのファイルクリック `open_file_row` がプレースホルダで待っている）。

- [x] 左サイドバー: cwd 連動ファイルツリー（FR-3.1 / FR-3.7。2026-06-12 完成。
      cmd+B トグル・cwd 追従・展開折りたたみ・2 秒ポーリング更新。
      ファイルクリックで開く動作は FR-3.2 のプレビューペインと同時に実装）
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

Phase 5 の技術選定（ハイライタ・Markdown レンダラ・git ライブラリ等）は、
候補 2〜4 個 + 推奨 1 つ + 各トレードオフ 1 行の形でユーザーへ提示し、承認を得てから採用する。

## Phase 5.5: tmux バックエンド永続化 → ✅ 完了（2026-06-12）

> 全 PTY を tmux session 化し、tako 再起動でセッション（実行中プロセス・画面内容）を
> **完全復元**する。FR-5 はこの方式で再設計済み（`requirements.md`）。
> 設計・罠は `architecture.md`「Phase 5.5」節。

- [x] 設計スパイク: spawn を `tmux new-session -A` 経由に差し替え（tmuxview / tty 突き合わせ・
      シェル統合（OSC パススルー）・kitty protocol（extended-keys）との共存を検証・実装。
      ZDOTDIR 消費・`display-message -p` 等の罠は architecture.md に記録）
- [x] 再起動時の再 attach とタブ / ペイン構成の対応付け復元（layout.json + 同一 ID 復元。
      `TAKO_PANE_ID` が再起動をまたいで有効 = AI 操作が途切れない）
- [x] tmux 不在環境では従来の直接 spawn へ無害に劣化（+ `tako persist` / MCP `tako_persist`
      で OFF 可能。設定は永続化）
- [x] tmuxview との整合: バックエンドセッションを `backend: true` + 保持ペイン / orphan で
      区別表示、kill は専用確認文言

**Exit Criteria**: tako を再起動しても、実行中のエージェント・dev サーバーが画面内容ごと戻ってくる。
→ 機械検証（core e2e: detach → 再 attach 復元 / OSC パススルー + セルフテスト 95 項目）は達成。
実 .app での再起動体験は manual-checks.md で常用確認。

## パネル UI 刷新（FR-2.16.4〜2.16.8）→ ✅ 完了（2026-06-12）

> 第2部としてユーザーから指示済みの実装タスク。仕様の正は `requirements.md` FR-2.16。

- [x] 下部ステータスバー新設（Zed / VSCode 風）: 左にファイルツリートグル、
      右に tmux 管理・git 管理のトグルボタン（git は git graph FR-3.6 実装まで
      プレースホルダビュー）。上部の「◧ panel」ボタンは廃止して集約。
      トグル状態の取得・操作は CLI / MCP からも（開発不変条件。ファイルツリーの
      CLI / MCP 経路は `tako panel --filetree` / MCP `tako_panel` の filetree として新設）
- [x] パネル内部タブの 1 本化: 現 agents ビューを「tmux」へリネームし、空表示バグのある
      旧 tmuxview を削除して統合。タブごとの「タブ名ラベル付き四角枠」+ 枠内に全ペインの
      入れ子表示。各ペイン行右にゴミ箱 → kill 確認 → kill（dispatch Close）。行は省略（…）で
      見切れさせない。セッション列挙の保証は旧 tmuxview 空表示バグの根治（bf37492 の
      ロケール注入）+ 統合ビューの e2e で担保
- [x] タブ未表示セッションのセクション（FR-2.16.8。実装中のユーザー追加要件）:
      「管理外」（ユーザー直起動等）と「kill漏れ?」（orphan バックエンド残骸）をラベルで
      区別して列挙し、どちらも確認つき kill（dispatch TmuxKill）

**Exit Criteria**: ◧ ボタン無しで全サイドバーがステータスバーから開閉でき、
パネルの tmux ビュー単独で「どのタブに何が居て、何が消し忘れか」が分かって殺せる。
→ 機械検証（セルフテスト 107 項目。統合ビューのタブ枠・ジャンプ・kill 確認フロー +
filetree CLI roundtrip）は達成。見た目は manual-checks.md で常用確認。

## 未表示の子の自動サーフェス（FR-2.18。フェーズ未定）

> spawn された子エージェント・サーバーがどのタブにも表示されていなければ tako が
> 自動で表示する（同タブ内分割等）。「未表示の子一覧」「指定した子を今のタブに表示」を
> MCP / CLI に公開。たまり場（FR-2.15）の**意図的な退避**は対象外（`requirements.md` FR-2.18）。

## localhost ポートパネル（FR-2.19。パネル UI 刷新後）

> FR-2.4.2 の listen ポート検知を情報パネルのビューに昇格。ポート / プロセス / PID /
> 対応ペイン / 管理外の区別 + 確認つき kill。「使用中ポート一覧」「空きポート提案」
> 「指定ポート kill」を MCP / CLI に公開（`requirements.md` FR-2.19）。
> 消し忘れ認知と AI の起動前ポート確認が目的。

## たまり場（FR-2.15。フェーズ未定・UI はユーザーと要相談）

> 「見た目はタブから消したいが処理は生かしたい」ターミナルのタブ外プール（2026-06-12 要望）。
> Phase 5.5 の「表示ペインを持たない生きたバックエンドセッション」構造が前提として整った。
> 着手前に見せ方（画面下部常設等）をユーザーと決めること（`requirements.md` FR-2.15）。

## Phase 6: Windows 本格対応

- [ ] ConPTY・named pipe・PowerShell シェル統合・ポート検知の Windows 実装を仕上げる
- [ ] Phase 0 で洗い出した GPUI Windows 未成熟箇所の再評価と回避策
- [ ] Windows でのフルシナリオ（Phase 3 の Exit Criteria 相当）達成

**Exit Criteria**: Windows ユーザーに「使ってみて」と言える品質。

## Phase 7: 公開準備（v0.1.0）

- [ ] MCP ゼロコンフィグオンボーディング（FR-2.14。MCP クライアント検出 + ワンクリック
      登録 + 診断表示 + instructions 品質整備。**セットアップ画面 = 自動診断チェックリスト +
      「セットアップ実行」ボタン一発自動導入（FR-2.14.6。claude CLI / MCP 登録 / tako CLI の
      PATH 設置）**。登録・診断・実行は CLI / MCP からも。2026-06-12 要望、配布前に必須）
- [ ] ネスト tmux の検出・診断・ワンタップ設定適用（FR-2.17。検出 = tty 突き合わせ →
      診断パネル表示 → 案内 + ボタン一発 / MCP 経由で `~/.tmux.conf` へ安全に適用。
      勝手に書き換えない。推奨スニペットは `tmux_backend::NESTED_TMUX_SNIPPET` が正、
      ネストチェーン e2e 2 本が品質保証。2026-06-12 要件化）
- [ ] **配布チャネル二本立て**（macOS。2026-06-12 方針決定）:
      ① **公式サイトからの DMG 直ダウンロード** = 初心者向けの主経路
      ② **Homebrew Cask** = 自前 tap（`takushio2525/homebrew-tako` 等）で先行運用 →
      安定後に本家 homebrew-cask へ PR。Cask の `binary` 指定で **tako CLI も PATH へ**設置。
      実体は GitHub Releases（macOS は notarization 必須）。winget（Windows）は Phase 6 後に検討
- [ ] **自動アップデート（必須要件）**: Sparkle 等の標準フレームワークで
      「アップデートが来ています」**通知 → 「アップデートして再起動」ボタンで自動適用**。
      手動 DMG 入れ直しはさせない。**brew / DMG どちらの導入経路でも全く同じ .app を配る
      単一アーティファクト方針**（Cask は `auto_updates true` を宣言し、アプリ内自動更新と
      `brew upgrade` を衝突させない）
- [ ] README・スクリーンショット・デモ GIF 整備
- [ ] リポジトリ公開（private → public 化）、CONTRIBUTING / Issue テンプレート

**Exit Criteria**: 公開して他人がインストールできる。

## v0.2 以降（公開後の後段フェーズ）

- [ ] AI 活動タイムライン: エージェントの実行コマンド・変更ファイル・コミットを
      時系列一覧するペイン（FR-2.11。監査可能性・信頼の土台）
- [ ] ~~セッション永続性: タブ / ペイン構成・cwd・タイトル・role の保存と復元（FR-5.1。
      シェル内容の完全復元はしない）~~ → **Phase 5.5（tmux バックエンド永続化）で完全復元
      として実現する**（2026-06-12 方針変更）
