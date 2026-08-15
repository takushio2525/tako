# architecture.md — 技術設計

> 「どう実現するか」を定義する。要件は `requirements.md`、実装順序は `roadmap.md`。

## 技術スタック

| 領域 | 採用 | 根拠 |
|---|---|---|
| 言語 | Rust | ネイティブ性能、メモリ安全、Warp / Zed / Alacritty の実績 |
| UI フレームワーク | **GPUI**（Zed 製、**zed リポ git rev 固定**） | GPU 描画で Zed 級の速度を出せる唯一級の Rust UI。Zed 本体が実証。バージョン戦略は下記 |
| ターミナルエミュレーション | **alacritty_terminal 0.26+**（crates.io） | 枯れた VT 実装（Apache-2.0、2026-04 更新で活発）。Zed のターミナルも同クレート採用 |
| PTY | **alacritty_terminal::tty**（Phase 0 で確定） | 同クレートが macOS openpty / Windows ConPTY を吸収済み。portable-pty は不要と判断 |
| 非同期 | **GPUI executor + futures channel**（Phase 0 で確定） | PTY IO は alacritty の EventLoop スレッド、UI への通知は channel + `cx.spawn` で足りる。tokio 不要 |

### ⚠️ 採用リスク（明記事項）

1. **GPUI は pre-1.0 であり、破壊的変更が頻発する。**
   Zed 本体の都合で API が変わる前提で付き合う。対策:
   - GPUI への依存を `ui/` レイヤに閉じ込め、コアロジック（ペインツリー・制御プレーン）は GPUI 非依存に保つ
   - **git rev 固定**（`rev = "..."`）で依存し、追従は意識的なタスクとして行う（自動更新しない）
2. **GPUI の Windows 対応**: Phase 0 の調査で「ビルド・起動の成立」リスクはほぼ解消と判断
   （下記「Phase 0 検証結果」参照）。残るのは品質面（スクリーンリーダー欠落等）と実機未検証であること。
3. **GPUI の汎用フレームワークとしての開発減速（2025 年末に Zed が表明）**。
   crates.io リリースは停滞しており、安定供給は期待できない。コミュニティフォーク
   gpui-ce（crates.io に 0.3.x あり、元 Zed 社員主導）が乗り換え先の保険。
   「ui/ に閉じ込める」方針がこのリスクの防波堤を兼ねる。
4. **ライセンス互換性**: GPUI は Apache-2.0、alacritty_terminal は Apache-2.0。
   ただし GPUI の推移的依存に zlog/ztracing（GPL-3.0-or-later）が含まれるため、
   tako 全体は GPL-3.0-or-later を採用。cmux のコードは引き続き絶対に読まない（`concept.md` 参照）。

### Phase 0 検証結果（2026-06-11、詳細は `poc/README.md`）

**結論: Rust + GPUI + alacritty_terminal スタックは成立。** macOS で最小ターミナル
（シェル起動・出力描画・キー入力）が動作した。PoC は `poc/` 配下（本実装とは分離）。

#### GPUI バージョン戦略: zed リポ git rev 固定を採用

- **crates.io 版（0.2.2）は 2025-10-22 以降更新停止**。開発減速宣言もあり再開は期待薄
- Windows まわりの修正・改善が入るのは git 版のみ → **git + `rev` 固定 + 意識的な追従**が唯一の現実解
  （gpui-component / Longbridge Pro など単体利用の先行事例も同方式）
- 検証時 rev: `cafbf4b5df7fedb67fc0f248850a5654efcec5d9`（2026-06-10 の main）

#### git 版 gpui のハマりどころ（実装時に必ず踏む）

- **`gpui_platform` の `font-kit` feature を有効にしないと文字が一切描画されない**（無警告でスタブ化）
- `Application::new()` 廃止 → `gpui_platform::application()`（プラットフォーム実装が別クレートに分離）
- 最新 stable Rust が必要（1.89 不可、1.95.0 で確認）。`rust-toolchain.toml` でピン留めする
- ウィンドウがオクルージョン状態だと display link が止まり再描画されない（仕様）
- `WindowHandle<V>::update` 内での `dispatch_keystroke` はビュー二重借用でパニック → `AnyWindowHandle::update` を使う
- IME（Phase 3.5 で実装。FR-1.9）: `Window::handle_input` は **paint フェーズ限定 API**
  → 何も描かない `canvas` の paint フックから `ElementInputHandler` を登録する。
  `on_key_down` で PTY へ書いたら **`cx.stop_propagation()` 必須**（未処理扱いだと macOS が
  キーを inputContext へ回送し insertText → `replace_text_in_range` で二重入力になる）。
  `StyledText::with_default_highlights` のハイライト範囲は**非重複・昇順**必須
  （重ねると `invalid text run` でパニック）。NSTextInputClient の範囲はすべて UTF-16 オフセット

#### GPUI の Windows 対応の現状（2026-06 時点、Web 調査ベース・実機未検証）

- **Zed 本体の Windows 版は 2025-10-15 に正式リリース済み**（DirectX 11 + DirectWrite のネイティブ実装、
  毎週リリースに組込み）。「Windows 対応は実験的」という認識はもう古い
- gpui 単体も Windows は feature flag 不要の公式サポート。gpui-component（★11k、Windows 対応明記）と
  その商用利用（Longbridge Pro）という git 依存単体利用の実績あり
- ビルド前提: MSVC C++ build tools + **Spectre-mitigated libs** + Windows 10 SDK 10.0.20348.0+ + CMake
- 既知の未成熟箇所（zed リポ issue、`platform:windows` 約 120 件）:
  - **スクリーンリーダー（UIA)対応が完全欠落**（#41138、未解決）— アクセシビリティ要件には致命的
  - フォント描画: Mica 有効時のぼやけ（#56382）、**ターミナル TUI のフォント描画崩れ（#58830）**、リガチャ（#51754）
  - テキスト入力時の GPU 負荷が高い（#37727）、画像アトラス解放漏れ（#56667）
  - IME（日本語含む CJK）は 2025 年末に集中修正されおおむね機能する模様
- **未検証**: この PC は macOS のため Windows 実ビルドは未実施。Phase 1 の CI 整備時に
  GitHub Actions の windows ランナーで PoC をビルド・スモークし、実機級の検証は Phase 6 で行う（`roadmap.md`）

## 全体レイヤ構成

```
┌─────────────────────────────────────────────┐
│ ui/        GPUI 依存はここだけ                  │
│  ターミナルビュー / タブバー / ペインレイアウト   │
│  サイドバー（ファイルツリー / git graph）        │
│  提案チップ                                    │
├─────────────────────────────────────────────┤
│ control/   制御プレーン（GPUI 非依存）           │
│  ipc サーバー（Layer1 CLI の受け口）            │
│  mcp サーバー（Layer2）                        │
│  detect（Layer3: OSC / listen ポート検知）      │
├─────────────────────────────────────────────┤
│ core/      ドメインモデル（GPUI 非依存）         │
│  Workspace / Tab / PaneTree / Pane            │
│  TerminalSession（alacritty_terminal + PTY）  │
├─────────────────────────────────────────────┤
│ platform/  OS 差分の吸収                       │
│  PTY 生成 / IPC トランスポート / プロセス監視    │
└─────────────────────────────────────────────┘
```

依存方向: `ui → control → core → platform`。逆依存・循環依存禁止。
**core と control を GPUI 非依存に保つ**ことが、GPUI 破壊的変更リスクの防波堤。

クレート分割（Cargo ワークスペース、Phase 1 で確定）:
`tako-core` / `tako-control` / `tako-app`（GPUI バイナリ）/ `tako-cli`（Layer1 CLI バイナリ）。

### ペイン矩形は「実描画のコンテナ矩形」が正（#684。2026-07-30）

ルートは flex 列で、上から タブバー / ウェルカムバナー（#549）/ アップデート通知カード（#616）/
**ペインエリア** / たまり場ドロワー / Web ビュー dock / ステータスバー が縦に積まれる。
ペイン自身はペインエリアの中で `absolute` + 単位矩形の百分率で置かれるので、実際の描画位置は
常にコンテナのレイアウト結果と一致する。一方 `render()` はレイアウト前に走るため、
そこでビューポート寸法から引き算して作る矩形は**推定**にしかならない。

この推定値は `pane_text_areas` を通じて PTY の行数・マウス座標→セル変換・カードの位置決めに
使われるので、引き算に入っていない要素が積まれた瞬間に「見えない行」が生まれていた（#684。
バナー表示中にヘッダは 119x27 と出しつつ実描画は 21 行）。

そのため**ペインエリアの実矩形を採取して次フレームの正とする**:

- ペインとまったく同じ指定（`absolute` + 原点 + `size_full`）の `canvas` をペインエリアの
  最初の子に置き、prepaint で採取した矩形を `PaneContentProbe`（ウィンドウ単位の `Cell`）へ書く。
  次の `render()` がそれを取り込んでペイン矩形の基準にし、診断用に
  `TakoApp::pane_content`（used / measured / corrections）へ写す
- **採取側でエンティティを触ってはいけない**: 描画は root view が貸し出されている最中に走ることが
  あり（`WindowHandle::update` の中から `Window::draw` を呼ぶ経路。セルフテストで実際に踏んだ）、
  そこで `Entity::update` を呼ぶと `cannot update ... while it is already being updated` で
  **プロセスごと abort する = 全ペイン消失**。`Cell` への書き込みだけなら借用に触れない
- 実測はコンテナのレイアウト結果だけに依存し、記録した値には依存しない（ペインは absolute なので
  コンテナの内在サイズに寄与しない）。よってずれは 1 フレームで収束する
- 描画中の `cx.notify()` は GPUI が捨てる（`WindowInvalidator::invalidate_view` は
  draw 中に dirty を立てない）ため、訂正フレームは `App::defer` 経由で起こす（貸し出しが
  解けた後に走るので安全）。同じ実測値では二度要求しない安全弁つき
  （収束しない経路が生まれても vsync ごとの再描画へ落ちない）
- 初回フレームだけ従来の引き算（推定）へフォールバックする

**新しく縦に積む要素を足すときに引き算の式を増やしてはいけない**（増やすと #684 が再発する）。
番犬テスト（`pane_content_geometry_tests`）が式の数・プローブの位置・
「プローブが描画中に entity を触らない」ことを固定している。

### ペイン**内部**の積み上げも同じ場所で会計する（#781。2026-08-06）

#684 が正にしたのは「ペインを**並べるコンテナ**」の矩形で、**ペイン内部**の積み上げは
別の話として残っていた。ペインもまた flex 列で、上から
タイトルバー（`PANE_TITLE_BAR`）/ **stale claude バナー（#498。`STALE_BANNER_HEIGHT`）** /
ターミナル領域（`flex_1`）/ AI コマンド提案カードの帯（#703）が積まれる。
スクロールバー・提案チップ・workers メニューは `absolute` なので高さを食わない。

テキスト領域の矩形（`pane_text_areas`）は

- PTY の cols / rows
- マウス座標 → セル変換（`cell_at` / `cell_at_clamped` = ドラッグ選択）
- IME のアンカー（未確定文字列の下線と変換候補ウィンドウ）

の**共通の正**なので、流れの中の要素を会計し忘れると 3 つが同時にずれる。
実際に stale claude バナーの 28px が漏れており、claude が自己更新した瞬間から
全 master / worker ペインで選択が 1〜2 行下に付き、IME がカーソルより上に出ていた（#781）。

そのため:

- 矩形は `pane_text_area_rect(content, unit_rect, stacked_top, band, scale_factor)` の
  **1 か所**で作る。`stacked_top` に「ペイン上端からテキストまでの積み上げ」を全部足す
- 高さの定数は描画側と会計側で**同じ定数**を共有する（`STALE_BANNER_HEIGHT`）。
  片方だけ変えるとずれるので、番犬テストが生の px 値での指定を禁じている
- ターミナル領域の実矩形も採取する（`PaneTextAreaProbe`。ペイン単位の `Cell`）。
  こちらは**正として使わない**（PTY の resize と結ぶと 1 フレーム遅れが行数の振動を生む）。
  「算術が実描画と一致しているか」の観測に限り、`render()` の冒頭で前フレームの算術と
  突き合わせて 1px 以上ずれていたら perf.log へ 1 回だけ自己申告する
  （コンテナ側が収束していないフレームは #684 の既知の過渡状態なので対象外）
- 機械検証はセルフテスト項目 106（バナー ON / OFF で算術 == 実描画、実描画左上が
  セル (0,0) へ解決する）と単体テスト `pane_text_area_tests`

## ドメインモデル

```
Workspace
└── Tab (= エージェントグループ。1 グループ = 1 タブ)
    └── PaneTree (二分木: Split { axis, ratio, children } | Leaf(Pane))
        └── Pane
            ├── TerminalPane (TerminalSession を保持)
            ├── PreviewPane (Code | Markdown | Pdf | Editor)
            └── WebViewPane (URL を表示。実現方式・リスクは「Web ビューペイン」節、後段フェーズ)
```

- `PaneId` / `TabId` はプロセス生存期間中ユニークな整数 ID（環境変数・CLI で使う）
- 自動生成ペインは必ず「呼び出し元 Pane が属する Tab」に挿入する（FR-2.1.2）
- Pane は `role`（任意ラベル）と `origin`（user / cli / mcp / suggestion）を持ち、
  UI 表示とポリシー制御（FR-2.3.5）に使う

### spawn レイアウトエンジン（FR-2.20、#165。2026-07-13 実装）

worker spawn の配置を「呼び出し元の右に等分割」から差し替えるレイヤ。

- **型とアルゴリズム**: `tako-core::spawn_layout`。`SpawnLayoutPolicy`
  （master-reserved 既定 / legacy）・`WorkerLayoutAlgorithm`（grid 既定 / spiral）・
  `SpawnLayoutConfig { policy, master_ratio, algorithm }` と、worker 領域サブツリーを
  組み立てる純関数（grid = 行優先の格子: rows = ceil(sqrt(n))・列等幅・列内等高、
  spiral = 先頭が半分を取り残りを直交軸で再帰分割。初回軸は Vertical = 上下）
- **PaneTree への適用**: `PaneTree::spawn_worker(anchor, pane, config)` と
  `PaneTree::reflow_workers(anchor, algorithm)`（`pane_tree.rs`）。
  **worker 領域 = anchor Leaf から根へのパス上で anchor と反対側にあり、全リーフの
  `spawned_by` チェーンが anchor へ到達するサブツリー**（最も近い祖先を優先）。
  領域が無ければ anchor を右分割して新設（anchor 側に master_ratio を残す）、
  あれば領域内の Pane を集めて理想形に再構築する（= anchor・領域外ペインの
  ratio / rect には一切触れない）。ユーザーペインが混在したサブツリーは領域と
  見なされないため、手動ペインの矩形が spawn / close で変わることはない
- **接続点**: spawn = `dispatch_orchestrator_spawn`（dispatch.rs）。close リフロー =
  dispatch `Request::Close` と tako-app `remove_pane_with`（UI × / exit 由来）の両方で、
  close 前に対象の role（orchestrator-worker）と spawned_by を記録 → close 成功後に
  `reflow_workers`。設定は `tako-control::setup::spawn_layout_config()`
  （config.yaml `spawn_layout` セクション。不正値・読み取り失敗は既定へフォールバック）
- **設定変更経路**: `dispatch_orchestrator_layout`（host 非依存）を dispatch
  `Request::OrchestratorLayout` と CLI `tako orchestrator layout` が共用（#83 の教訓）。
  MCP は `tako_orchestrator_layout`（計 59 ツール）
- **注意**: `spawned_by` は永続化されない（セッション内使い捨て）。tako 再起動後の
  spawn は既存 worker を領域と認識できず新設パスに落ちる（許容。worker はタスク単位で
  使い捨てる運用のため）。grid の列数は n ≤ 100 で ratio ≥ 0.1 に収まり MIN_SHARE
  クランプと整合する

### ⚠️ PTY セッション破棄のハマりどころ（2026-06-11 常用クラッシュの教訓）

- **alacritty に既定シェル解決を任せない**（`tty::Options.shell = None` 禁止）。macOS では
  setuid root の `login` ラッパ経由になり、ペイン close 時の `Pty::drop` が
  `kill(login, SIGHUP)` を権限エラーで失敗（返り値無視）→ `child.wait()` 永久ブロック →
  **close のたびに master fd・signal fd・IO スレッド・login プロセスがリーク**する。
  本家 alacritty はウィンドウ close = プロセス終了のため顕在化しないが、tako はペイン単位で
  セッションを破棄するので直撃する（fd 枯渇 → PTY 生成失敗）。
  tako は `$SHELL` をユーザー権限で直接 spawn する（`TerminalSession::default_shell`、`-l` 付き）
- **PTY 生成失敗で panic しない**。GPUI のイベント処理は FFI コールバック内のため、
  panic は unwind できず SIGABRT でアプリごと落ちる。`spawn_session` は Result を返し、
  失敗時はペインを巻き戻して CLI / MCP へエラー応答する。
  回帰はセルフテスト 40 / 40b（split→close ストレス + fd リーク検査）で機械検証する

### PTY IO ループは tako 側に持つ（`tako-core::pty_loop`。#817）

alacritty_terminal の `EventLoop` は使わず、**同等のループを tako が持つ**。理由は 1 点だけ:

upstream は reader スレッドの**スタック**に `[0u8; READ_BUFFER_SIZE]`（1 MiB）を置く。
ゼロ初期化なのでスレッド開始時点で全ページが dirty になり、**ペイン 1 枚 = 約 1.03 MB が常駐**
していた（16 ペインで stack 17 MB。#814 の実測）。`READ_BUFFER_SIZE` は `pub(crate)` で
外から下げられず、`Builder::stack_size` を絞っても memset された分は reserve ではなく
resident なので減らない（減るのは仮想サイズだけ）。**バッファをヒープへ動かすには
ループ自体を持つしかない**。

移植にあたっての約束:

- **挙動は upstream と同一に保つ**。ロック粒度（`MAX_LOCKED_READ` = 64 KiB）、
  バッファ上限に達したときのブロッキングロック（= PTY のバックプレッシャ特性）、
  シャットダウン順序（`Msg::Shutdown` → ループ脱出 → `deregister`）を変えない
- 読み取りバッファは 64 KiB 始まり。ロックが取れている通常経路は `MAX_LOCKED_READ` で
  打ち切られるので **read / parse の回数は upstream と同じ**。足りないとき（ロック競合中に
  64 KiB を超えて届いたとき）だけ上限 1 MiB まで倍々で伸ばし、`pty_read` の最後に戻す
- poller のトークンは Windows では upstream が `pub` で出しているものを使い、Unix だけ
  `pub(crate)` の値を写す。**写し違いはハングとして現れるので単体テストで潰す**
  （実 PTY を張って読み取り / 子プロセス終了イベントのキーを確認する。値を壊すと
  ハングではなく FAILED になるところまで作ってある）
- 由来と改変内容の告知は `THIRD-PARTY-NOTICES.md`（Apache-2.0 → GPL-3.0-or-later は一方向互換）

alacritty_terminal を上げるときは `event_loop.rs` の差分を見て、上の 3 点（ロック粒度・
上限到達時の挙動・トークン）に変更が無いかを確認すること。

## 制御プレーン（コンセプト①の 3 層）

### 環境変数注入（共通基盤）

TerminalSession がシェルを spawn する際に注入する:

| 変数 | 内容 |
|---|---|
| `TAKO_PANE_ID` | 呼び出し元ペインの ID |
| `TAKO_TAB_ID` | 所属タブの ID |
| `TAKO_SOCKET` | IPC エンドポイント（macOS: Unix domain socket パス、Windows: named pipe 名） |
| `TAKO_MCP_URL` | 内蔵 MCP サーバーの接続先（Layer2 自動発見用。**Phase 3 で注入開始**） |
| `TAKO_TOKEN` | 接続認証トークン（セッション毎に生成。外部プロセスの接続拒否に使う） |

Phase 2 時点では `TAKO_MCP_URL` 以外の 4 つを `TerminalSession::spawn`（`SpawnOptions.env`）
経由で注入済み（tako-app の `spawn_session`）。

### Layer 1: CLI（`tako-cli`）→ ✅ 実装済み（Phase 2、2026-06-11）

- 単一バイナリ `tako`。`TAKO_SOCKET` + `TAKO_TOKEN` を読んで IPC サーバーに JSON-RPC で接続
- pane 指定省略時は `TAKO_PANE_ID` を呼び出し元として使う（FR-2.2.7）
- `TAKO_SOCKET` が無ければ「tako の外で実行されている」エラー（FR-2.2.8）
- サブコマンド: `split` / `send` / `focus` / `list` / `read` / `close` / `title` /
  `resize` / `equalize` / `tab new` / `tab select` / `tab move-pane`（カタログは FR-2.5）
- IPC プロトコルは MCP ツールと同じ操作セットに 1:1 対応させ、実装を共有する

### IPC トランスポート（Phase 2 実装メモ）

- ワイヤ形式: **1 行 1 JSON の JSON-RPC 2.0 サブセット + `token` フィールド拡張**
  （`crates/tako-control/src/protocol.rs` が正）。操作セットは FR-2.5 と 1:1
- **操作のディスパッチは `tako-control::dispatch`（`ControlHost` trait）に一元化**。
  tako-app の IPC 受信ループと Phase 3 の MCP サーバーが**同じ dispatch を呼ぶ**ことで、
  設計原則 5（UI でできることはすべて AI からもできる）のセマンティクスを一箇所に保つ。
  ControlHost は 8 つの責務別サブトレイト（`WorkspaceHost` / `SessionHost` / `TmuxHost` /
  `UiStateHost` / `PreviewHost` / `WebViewHost` / `RemoteHost` / `SystemHost`）のスーパー
  トレイト（blanket impl で合成。`host.rs`）。dispatch のシグネチャ `&mut dyn ControlHost`
  は不変（Issue #86）
- unix: `$TMPDIR/tako-<pid>-<seq>.sock`（パーミッション 0600）+ 32 バイト CSPRNG トークン
  （getrandom）。accept スレッド + 接続毎スレッドのブロッキング IO で受け、リクエストは
  futures channel で UI スレッドへ渡して dispatch する（tokio を持ち込まない方針を維持）
- CLI / MCP からの `close` は「最後のタブの最後のペイン」を拒否する
  （アプリ終了に等しい操作は UI の cmd+W のみ。FR-2.5.9 の安全性方針）
- **接続情報の永続化と発見（FR-2.2.9、2026-06-12）**: アプリ起動時に
  `<data_dir>/control.json`（0600 / 親ディレクトリ 0700。tmp + rename で原子的に更新）へ
  socket / token / mcp_url を書き出す（`tako-control::discovery`）。CLI は
  環境変数 → 発見ファイルの順で解決し、env があっても**接続不可・認証失敗のときだけ**
  フォールバックする（操作エラーはそのまま返す）。ソケットパスは PID 入りのまま
  （安定パスは複数インスタンスで取り合いになるため不採用）。複数インスタンスは
  最新起動がファイルを上書き = 最新優先。終了時の削除はしない（GPUI の終了経路で
  Drop が保証されない。残骸は接続失敗として顕在化し、誤接続はトークンで防がれる）
- **TODO(Phase 6): Windows named pipe**。`IpcServer::start` と CLI の transport は
  `#[cfg(windows)]` でスタブ化済み（サーバー起動失敗でもアプリは IPC なしで継続する）。
  実装時の検討事項:
  - パイプ名規約（`\\.\pipe\tako-<pid>` 想定）を `TAKO_SOCKET` に入れる
  - アクセス制御: UDS の 0600 に相当する DACL（同一ユーザー限定）+ トークン認証の二段
  - ConPTY 環境での env 注入は alacritty_terminal の `tty::Options::env` 依存のため実機確認

### Layer 2: 内蔵 MCP サーバー → ✅ 実装済み（Phase 3、2026-06-11）

#### 構成: トランスポート非依存エンジン + 2 トランスポート

- **MCP エンジン**（`tako-control::mcp`）: initialize / tools/list / tools/call の JSON-RPC
  処理とツールカタログ。**実行は IPC と同じ `dispatch` を呼ぶだけ**（操作セマンティクスの
  一元化 = 設計原則 5。MCP 固有の操作実装はゼロ）
  - `mcp/mod.rs`: 公開ファサード、JSON-RPC、UI スレッドで実行できない special handler
  - `mcp/catalog.rs`: 133 ツールの名前・説明・入力スキーマ（公開契約の正）
  - `mcp/request.rs`: MCP 引数から `protocol::Request` への変換と入力検証
  - `mcp/http.rs`: localhost HTTP の認証・受信・応答
  - `mcp/tests.rs`: エンジン / Request 変換 / HTTP の単体・往復テスト
  公開 API（`tools` / `handle_message` / `McpSession` / `McpServer`）はファサードから維持する。
  全カタログ JSON は `testdata/mcp_tools_full_snapshot.json` で順序を含め固定し、さらに全ツール名が
  `Request` 変換か明示 special handler のどちらかへ到達することを網羅テストで保証する。
  新ツール追加時の編集先は catalog / request（長時間処理だけ mod.rs の special handler）へ限定し、
  HTTP・stdio・dispatch・CLI の接続コードへ同じ定義を重複させない。
- **トランスポート 1: Streamable HTTP**（`McpServer`、tako-app に内蔵）:
  127.0.0.1 の空きポートに tiny_http（同期・スレッドベース。tokio を持ち込まない方針を維持。
  公式 SDK rmcp は tokio 必須のため不採用）で立て、URL を `TAKO_MCP_URL` として全ペインへ注入。
  認証は `Authorization: Bearer <TAKO_TOKEN>`（IPC とトークン共有）+ Origin ヘッダ検証
  （非 localhost は 403。DNS リバインディング対策）。POST のみ実装（GET の SSE ストリームは
  サーバー発信を持たないため 405）。**Windows でも動く**（named pipe 未実装の Phase 6 まで、
  Windows のエージェント連携は HTTP 側が受け皿になる）
- **トランスポート 2: stdio ブリッジ**（`tako mcp serve`、tako-cli）:
  stdin/stdout で MCP を話し、実行だけ IPC へ `origin="mcp"` で中継する。
  接続情報は**起動される度に** `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` を環境変数から
  読む（エージェントの子プロセスとして起動されるため、ペインのシェル → エージェント →
  ブリッジと環境が継承される = 呼び出し元ペインの特定が自動で成立する）。
  tako の外で起動された場合は **0 ツール**を返して無害化する

#### Claude Code「設定ゼロ接続」の検証結果（2.1.172、2026-06-11）

- Claude Code には**環境変数だけから MCP サーバーを自動発見する機構が無い**。
  登録経路は `.mcp.json`（プロジェクト）/ user・local スコープ設定 / `--mcp-config` フラグのみ
- プロジェクト `.mcp.json` の自動生成案は不採用: ユーザーのリポジトリを汚す・承認プロンプトが
  出る・URL とトークンがセッション毎に変わり静的ファイルと相性が悪い
  （`.mcp.json` は `${VAR}` 展開を持つため `${TAKO_MCP_URL}` 参照は可能だが、
  tako 外で開いたときに壊れた設定として残る）
- **採用: user スコープへの stdio ブリッジ登録（初回 1 回だけ）**

  ```sh
  claude mcp add --scope user tako -- /path/to/tako mcp serve
  ```

  以後はどのプロジェクト・どのペインでも設定なしでペイン操作ツールが使える。
  ブリッジが毎回環境変数を読むため URL / トークンの変動に強く、tako 外では 0 ツールで邪魔しない
- 実機検証は `scripts/verify-claude-mcp.sh`（GUI なしで IPC + MCP + dispatch を立てる
  `tako-control` の example `mcp_host` 内で実物の `claude -p` を実行。stdio / HTTP の両経路。
  ユーザーのグローバル claude 設定は変更しない `--mcp-config --strict-mcp-config` 方式）

#### 基盤となる公開ツール（Phase 3 で実装した 12 個。現在の全 133 ツールは完全スナップショットで固定）

`tako_list_panes` / `tako_split_pane` / `tako_send_input` / `tako_read_pane` /
`tako_focus_pane` / `tako_close_pane` / `tako_resize_pane` / `tako_equalize_layout` /
`tako_set_title` / `tako_create_tab` / `tako_select_tab` / `tako_move_pane_to_tab`

- 誤用しにくさを優先した設計: `tako_send_input` / `tako_read_pane` は **pane 必須**
  （省略で自ペインへ誤送信する事故を防ぐ）。`tako_close_pane` は pane 省略 = 自己片付け
  （FR-2.5.4）。スキーマは `additionalProperties: false` + enum で締める
- initialize の `instructions` とツール説明文に FR-2.7.5 の行動規範を埋め込み済み
  （レビューを求めるときは見せろ / 読んでほしければ開け / 方針相談は例を作って並べろ /
  終わったら片付けろ / 操作前に list で現状把握）
- 後段フェーズの追加ツール（案）: `tako_open_file`（プレビュー表示、FR-2.5.11）/
  `tako_open_url`（Web ビュー、FR-2.5.12）/ `tako_annotate`（注釈オーバーレイ、FR-2.6）/
  `tako_show_file` / `tako_show_diff` / `tako_show_url`（AI 成果物プレゼンテーション、FR-2.7）
- 呼び出し元ペイン特定（FR-2.3.3）: stdio = `TAKO_PANE_ID`、HTTP = `X-Tako-Pane` ヘッダ。
  pane 省略時のデフォルト対象が呼び出し元（= 同タブ）になる。ハードなスコープ強制は
  FR-2.3.5 のポリシー制御と併せて後段

### Layer 3: パッシブ検知 → OSC 7/133 は実装済み（2026-06-11）

- **OSC 7**（cwd 通知）→ ファイルツリーの cwd 連動（コンセプト②でも使う）
- **OSC 133**（プロンプトマーク）→ コマンド単位の区切り・実行中/完了の把握
- シェル統合スクリプトは zsh / bash / fish / PowerShell を同梱し、可能な範囲で自動注入
- **実装メモ（2026-06-11）**: vte は OSC 7/133 を unhandled で捨てるため、
  `EventedPty` の委譲ラッパ `TapPty`（`tako-core::osc_tap`、分割読み耐性スキャナ）で
  PTY 読み取りバイト列を EventLoop 手前で観測する（バイト列は不変更）。
  検知は `SessionEvent::Osc` → `TerminalSession` の cwd / `CommandState` へ反映し、
  dispatch の list（CLI / MCP）に `cwd` / `state` / `exit_code` として公開。
  シェル統合の注入は `tako-core::shell_integration`（zsh = ZDOTDIR / bash = PROMPT_COMMAND /
  fish = XDG_DATA_DIRS の 3 点セットを判定なしで常時注入。
  `TAKO_NO_SHELL_INTEGRATION=1` で無効化。PowerShell は Phase 6）
- **listen ポート検知** → 検知層は実装済み（2026-06-12、FR-2.4.2。`tako-core::ports`）
  - macOS: libproc（`proc_listpids` → `proc_bsdinfo.e_tdev` とペインの PTY スレーブ rdev の
    突き合わせで「ペイン配下」を判定 → `PROC_PIDLISTFDS` + `PROC_PIDFDSOCKETINFO` で
    LISTEN 中 TCP を列挙）。libc に無い `socket_fdinfo` 系は SDK ヘッダから転記し、
    **自プロセスで実際に listen するユニットテストで ABI を e2e 検証**している。
    Windows: `GetExtendedTcpTable`（Phase 6）
  - ポーリング方式（3 秒）。スキャンは background executor、結果は TerminalSession に保持し
    list / MCP の `listen_ports`（port / pid / process）として公開
- 検知結果は**提案チップ**（「localhost:5173 をプレビューで開く？」）として UI に出すだけ。
  承諾時のみペイン生成（強制分割はしない）。設定で全体を無効化可能（FR-2.4.4）

## Phase 5.5: tmux バックエンド永続化（FR-5。2026-06-12 実装）

全ペインの PTY を tako 専用 tmux サーバー（`tmux -L tako`。ユーザーの既定サーバーとは
分離・専用 conf でユーザーの `~/.tmux.conf` は読まない）のセッションとして保持し、
再起動時に attach し直して実行中プロセス・画面内容ごと完全復元する。

- **spawn 経路**（`tako-core::tmux_backend::wrap_options`）: シェル直接 spawn の代わりに
  `tmux -L tako -f <conf> new-session -A -D -s tako-<rand>` を PTY 子プロセスにする。
  `-A` で「新規作成」と「再 attach」が同一コマンド（消えていたセッションは `-c` の
  保存 cwd で開き直しになる）。`-D` で多重起動時は最新インスタンスへ収束
- **レイアウト**（`tako-control::layout` → `<data_dir>/layout.json`）: タブ / 分割ツリー /
  タイトル / role / cwd / セッション名を**ペイン・タブ ID ごと**保存し、復元時は同じ ID を
  再現（`Pane::restore` 等が採番カウンタを fetch_max で先へ進める）。これで tmux 内で
  生き続けるプロセスの `TAKO_PANE_ID` / `TAKO_TAB_ID` が再起動後も有効。旧 socket/token は
  CLI / MCP ブリッジの control.json フォールバック（FR-2.2.9）が吸収する。
  保存は 2 秒ポーリング + dispatch 後 + cmd+Q 時（差分時のみ書き込み）
- **PC 再起動時の Claude 会話復旧**（Issue #139）: `tako-control::agents` が
  `claude agents --json` を 1 回取得し、tmux `pane_pid` への祖先照合で確定した
  backend session → Claude session ID 対応を 5 秒ごとにバックグラウンド更新する。
  `layout.json` の各ペインへ session ID を保存し、復元時は backend tmux session が
  **存在しない場合だけ** transcript の存在・ID 形式を確認して、新規ログインシェルの PTY へ
  `claude --resume <session-id>` を投入する。backend が生存する通常の tako 再起動では
  再 attach のみで二重起動しない。検出成功時に一覧から消えたペインの関連は削除し、
  ユーザーが明示終了した古い会話を次回 PC 起動で勝手に戻さない。制御は既存 persist 設定を共有
- **tmux 不在時の劣化と診断**（Issue #30。2026-07-02）: レイアウトの保存・復元は
  **tmux が無くても機能する**（PTY のみ直接 spawn に劣化。保存時は `session: None` +
  cwd、復元時は保存 cwd で新シェルを開く「構造のみ復元」）。かつては保存・復元とも
  `tmux_backend::available()` にゲートされており、tmux 未導入の配布先（Homebrew cask は
  tmux を依存に含まない）で **persist 全体が無音で不活性化**していた。結果は
  `<data_dir>/persist.log`（`tako-control::diag`。復元成否・理由・明示削除を記録、
  256KB で `.old` ローテート）と `tako persist` / MCP `tako_persist` の
  `layout_path` / `layout_exists` / `last_restore` / `log_path` で診断できる。
  破損 layout.json は上書きせず `layout.json.corrupt` へ退避する
- **close 整合**: 明示 close（×・cmd+W・CLI / MCP close）= バックエンドセッションも kill。
  アプリ終了・クラッシュ = detach のみ（= 永続化）。PTY 死亡由来の close
  （SessionNotice::Exited。`CloseReason` で明示 close と区別）はセッションを kill せず、
  全滅時も layout.json を保持する（Issue #30。2026-07-03 実機: サーバー死で全タブ道連れ）。外部 tmux に attach しただけの
  ペインは何も kill しない（kill 対象は `backend_sessions` 登録分のみ）。詳細は
  `requirements.md` FR-5 の close 整合節
- **共存のための conf**（`<data_dir>/tmux-backend.conf`、毎起動再生成）:
  `status off` / `prefix None`（tmux の UI・キー介入ゼロ）、`mouse on`（マウス要求
  アプリへの SGR 生転送に必要。**非マウスペインのスクロールは SGR ではなく
  tako 自身が copy-mode を駆動する** → 下記「スクロール制御」）、`allow-passthrough on`、
  `extended-keys always` + `terminal-features extkeys`（kitty / CSI u 維持）、
  `history-limit 10000`、`update-environment TAKO_*`、
  `copy-mode-position-format ''`（copy-mode 右上の位置インジケータを消す。tmux 3.6 の
  既定書式は先頭行タイムスタンプ = **時刻表示**を含み、スクロール中に謎の時計として
  見える実機バグ (2) の正体だった。位置は tako 側スクロールバーが示す）。
  **conf はサーバー起動時にしか読まれない**ため、稼働中サーバーへは起動時に
  `tmux_backend::sync_conf`（`source-file`）で再適用する（下記の罠）
- **シェル統合の共存**: tmux は OSC 7 / 133 を外へ転送しないため、統合スクリプトが
  バックエンド配下（`$TMUX` のソケット basename が `tako*`）では OSC を
  `\ePtmux;…\e\\` パススルーで包む。同時に **TMUX / TMUX_PANE を unset** し、
  ユーザー自身の `tmux` 利用（ネスト）を素通しにする（バックエンドは見えない裏方）
- **tty 突き合わせの維持**: ペイン配下プロセスの制御端末はバックエンドサーバー側の
  ペイン tty になるため、spawn 後に `list-panes -t =<session> -F '#{pane_tty}'` で解決して
  `TerminalSession::set_tty_name` で差し替える（listen ポート検知 FR-2.4.2 と
  tmuxview FR-2.13.2 が引き続き機能する）

### スクロール制御（`tako-core::scroll`。2026-06-12 夜に方式転換）

> 2026-07-13 #159 で**スクロールバック表示はローカルミラー方式**
> （`tako-core::scroll_mirror`。capture ベース、copy-mode 非使用）へ再転換した。
> 表示系の現行仕様は `requirements.md` FR-2.5.13 と `scroll_mirror.rs` の
> モジュールコメントが正。本節の copy-mode 駆動の記述は Nested 解決・
> ターゲット表記・罠の記録として残す

バックエンドペインのスクロールバックは tmux 側にあり、ユーザーが自前 tmux を
ペイン内で attach していれば**ネスト先サーバー**にある。当初は SGR ホイールを
流し込んで tmux 既定バインドの copy-mode に任せていたが、実機で
① 1 イベント = 5 行で「ばっ」と飛ぶ ② copy-mode に入りっぱなしでキー入力が
飲まれる ③ copy-mode カーソルが画面に居座る、の 3 症状が出た。現方式:

- **解決**: `scroll::resolve_target` がペインの tty とネスト候補サーバーの
  `list-clients` を突き合わせ、実体（Backend / Nested）を特定する
- **駆動**: `scroll_by` / `scroll_to` が `copy-mode -e` + `send-keys -N n -X
  scroll-up/down` を正確な行数で発行（履歴ゼロでは copy-mode に入らない）。
  ターゲット指定は `=セッション名:`（**末尾コロン必須**。`=name` 単体は
  "can't find pane" になる）
- **出し分け**: 対象ペインの `mouse_any_flag` が立っていれば（vim / claude 等）
  レポートを tmux サーバーへ直接注入（`send-keys -H`。#167、下記節）。
  それ以外は tako 駆動（#159 以降はローカルミラー）

### マウスレポート転送（#167。2026-07-13 実装）

マウス要求 TUI（claude 等）へのホイールレポート転送で、SGR 断片
（`4;45;18M` / `<64;12;17M`）がテキストとして内側の入力欄へ混入する事故の根治。
実測（隔離 tmux + 実 claude）で確定した機序:

- 外側クライアント PTY へ書いた SGR シーケンスが**途中で 10ms 以上途切れる**と、
  tmux（escape-time 10ms）が ESC を単独キー確定し、残りを平文として内側へ
  転送する（`\x1b[<6` + 600ms + `4;45;18M` で入力欄への `4;45;18M` 混入を再現）
- 途切れの実態は、洪水（慣性スクロール = 数百〜数千イベント/秒）で PTY
  書き込みバッファが詰まったときの部分 write と、書き込み再開の停滞
  （UI / イベントループのストール。perf.log に 0.8 秒級の実績 #113/#115）

対策は二層:

1. **バックエンドペインは外側 PTY にレポートを書かない**:
   `scroll_mirror::send_wheel`（`tmux send-keys -H`、16 進バイトの直接注入）で
   サーバーへ渡す。ソケット越しの構造化データのため、分割・escape-time・
   tty_keys パースと無縁（断片化が構造的に起きない）。形式は
   `#{mouse_sgr_flag}`（`history_state`）で SGR / X10 を出し分け、UI 層
   （`pump_wheel`）が in-flight 1 本に直列化する（順序保証 + サブプロセス
   起動レートの自動調整）
2. **転送レート制限**（`terminal.rs`、全ペイン共通）: トークンバケット
   150 イベント/秒・バースト 8。直接ペインの PTY 転送と send-keys の起動
   レートを抑え、停滞時に飛行中のバイト量が escape-time 事故を起こす規模に
   ならないよう保つ。超過イベントは捨てる（ホイールは相対量のため無害）。
   150/秒は実 claude（tmux 越し・busy 中）で断片ゼロの実測値、バースト 8
   （約 104B）は macOS PTY 書き込みバッファ（1024B）に対する安全マージン

e2e: `tmux_backend::マウスレポート洪水でも断片がテキスト化しない`
（断片判定 + レート制限の存在）/
`scroll_mirror::ホイールレポートのtmux直接注入が内側に生で届く`（注入経路）。
断片判定の capture は `-J`（折り返し結合）必須 — 80 桁折り返しで intact な
レポートが行を跨ぎ、断片と誤判定する（実装中に踏んだ罠）
- **UI 側**（`ScrollCtl`）: ホイールは pending に積んで 1 つの tmux 操作へ
  コアレッシング（洪水対策）。キー入力・IME 確定の前に `cancel` を同期実行して
  iTerm2 流「打ったら最下部へ戻って入力」。copy-mode 中はカーソル強調を抑止
  （`screen_opts`）。スクロールバーは tmux の position / history を表示し、
  スクロール中だけ表示 → フェードアウト
- **CLI / MCP**: dispatch の `Scroll` が同じ `tako-core::scroll` を呼ぶ
  （開発不変条件）。応答を UI が `sync_scroll_from_dispatch` で取り込み、
  AI のスクロールでもバーが出る

**2026-07-13（#159）に copy-mode 駆動 → ローカルミラー方式へ再転換**: スクロール開始時に
`capture-pane -e` で履歴をチャンク取得（`tako-core::scroll_mirror`）し、以降の描画は完全
ローカル（copy-mode 不使用）。詳細は `requirements.md` FR-2.5.13。`resolve_target` /
`mouse_any_flag` の出し分け・`=セッション名:` の罠は現方式でも同じ。

**ミラー経路の対象判定は `mirror_scroll_pane`（backend_sessions ∪ tmux_view_panes）**
（2026-07-13、#181）: `tako tmux open` の TmuxOpen ビューペイン（再アタッチ・`tako-view-*`
ラッパー）は外側 alacritty が alt screen（履歴なし）のため、`backend_sessions` だけで分岐すると
直接ペイン扱いに落ちてスクロール不能になる（#177 復旧直後の実機で全ペイン非機能に見えた根因）。
TmuxOpen ペインの実体は wrapper（無ければ元セッション）@ socket を `ScrollTarget::Nested` として
resolve なしで確定する（`MirrorSource::Fixed`）。実体解決は**ビュー先優先**: persist ON では
ビューペインの外側 PTY 自体が backend ラップされ `backend_sessions` にも載る（実測: ラッパーの
client_tty = backend セッションの pane_tty）ため、backend を先に見ると外側（history 0）へ
誤解決する。また persist 復元で戻ったビューペインは `tmux_view_panes` に載らないため、
`resolve_target` のネスト候補を `[None]` → `[None, backend socket]` に広げ、`--socket tako`
（README の #177 復旧手順）のビュー先を tty 突き合わせで検出する。カスタム `-L` 外部サーバーの
ビューだけは復元後に検出不能（既知制約。開き直せば回復）

### ⚠️ スパイクで踏んだ罠（再発防止）

- **tmux に明示コマンドを渡すと `default-shell -c <cmd>` で実行される**: この非対話 zsh
  ラッパーが tako のシェル統合 .zshenv を読んで ZDOTDIR を消費し、内側の対話シェルに
  統合が届かなくなる。**既定シェル起動ではコマンドを渡さず** tmux の default-shell
  （ログインシェル直接 spawn）に任せる
- **`"${var//$'\e'/$'\e\e'}"` はダブルクォート内で置換側の `$'…'` がリテラル**になる
  （zsh / bash 共通）。ESC の二重化は `local esc=$'\e'` を経由する
- **`display-message -p` はクライアント無しだと空を返す**。detached セッションの
  pane_tty 取得は `list-panes -F` を使う
- **conf（`-f`）はサーバー起動時にしか読まれず、サーバーは tako の再起動を生き残る**:
  バージョン更新で conf を変えても既存サーバーには永久に届かない。起動時・persist
  有効化時に `sync_conf`（`tmux source-file`）で再適用する（e2e:
  `sync_confは稼働中サーバーへ設定を再適用する`）。サーバー不在時の `source-file` は
  エラー終了するだけでサーバーを自動起動しない（検証済み）
- 検証は `tako-core::tmux_backend` の e2e テスト（detach → 再 attach の内容復元 /
  OSC 7 パススルー）+ セルフテスト 58〜62（隔離ソケット `TAKO_TMUX_SOCKET`）で機械化済み

### ⚠️ 実機リグレッション（2026-06-12 常用報告）と恒久対策

- **Dock 起動の .app は PATH が最小構成**（/usr/bin:/bin:…）で Homebrew の tmux が
  見えない → tmuxview が空 + バックエンドが沈黙劣化 + 明示コマンド split が PTY 失敗。
  対策: ① `tmux::tmux_bin()`（`TAKO_TMUX_BIN` → PATH → 既知の場所 → ログインシェル
  `command -v` の順で解決・キャッシュ）を全 tmux 呼び出しで使う。
  ② 明示コマンドは `terminal::login_shell_command`（`$SHELL -l -c "…"`）で包んで spawn する
  （ユーザーの PATH・環境で実行。直接 exec しない）
- **Dock 起動の .app はロケール環境変数もゼロ → tmux クライアントが C ロケールになり、
  tmux 3.6 はコマンド出力中の制御文字を `_` にサニタイズする**: `-F "…\t…"` の
  タブ区切り出力が `master-2_1781179563_0` になり、tako 内の**全 tmux パースが沈黙全滅**
  （tmuxview 空表示 + tako 駆動スクロール無反応の共通根本原因。kill 系だけ動くのは
  フォーマット出力を使わないため。シェルから叩くと UTF-8 ロケールで再現しない罠）。
  対策: `tmux::tmux_command()`（`LC_CTYPE=UTF-8` 注入 + `LC_ALL` 除去）を全 tmux
  子プロセスの唯一の入口にする。ペイン側 CJK 対策（LC_CTYPE 既定注入・P0）と同型。
  e2e: `ロケール無し環境でもタブ区切り出力が壊れない`（C ロケールで `_` 化する
  カナリア + 注入後の TAB 保持）
- **マウスレポートの保証**（tako の存在意義。Zed の同症状が自作の動機）:
  「内側アプリがマウスを要求したら必ず生の SGR イベントが届く」「alt-screen 非マウス
  ペインへのホイールが矢印キーに化けない」を core e2e で常時検証する。
  バックエンド `mouse on` で claude（マウス非要求・通常画面）へのホイールは tmux
  copy-mode = チャットが遡れる
- **修飾付きキー（Shift+Enter 等）の CSI u 送出は全ペイン常時有効**
  （`CsiUMode::ModifiedOnly` が既定。Issue #28 で backend 限定 → 全ペインに変更）:
  修飾付き Enter はレガシー形式だと素の `\r` に潰れて区別不能な一方、Claude Code は
  kitty protocol を要求・クエリせずとも CSI u 入力を解釈する（2026-07-02 v2.1.198
  素の PTY で実測）。内側の kitty 要求は tmux から外側端末へ伝わらない（内側が要求
  しても外側 Term の DISAMBIGUATE は立たない）ため「要求を見てから送る」は不可能で、
  常時送出が正。tmux バックエンドペインは extended-keys always + csi-u 形式が内側へ
  届け、直接 spawn ペイン（tmux 無し環境 = Homebrew 配布の既定）はそのまま届く。
  旧実装は backend 限定だったため tmux 無し環境で Shift+Enter 改行が死んでいた。
  CSI u 非対応アプリ（素の zsh 等）では修飾付き Enter が「3;2u」風の文字列になるが、
  backend ペインは 2026-06-12 から同挙動で実害報告なし（受容済みトレードオフ）。
  ただし **Esc 単押しは CSI 27u にせず素の `\e` を送る**: tmux 3.6 は受信した
  CSI 27u を内側ペインの kitty 要求の有無に関係なく素通しする（extended-keys
  on / always どちらでも。実測）ため、CSI u 非対応アプリ（素の zsh、kitty を
  pop 中の claude 等）の入力欄に「27u」が文字として挿入される（2026-06-12
  実機バグ）。素の `\e` は escape-time で正しく解釈され内側へ素のまま届く。
  Esc を CSI 27u で送るのはアプリ自身の kitty 要求を外側 Term が直接見た場合
  （`CsiUMode::Full`）のみ。往復 + 「27u」非漏出は e2e 済み、Shift+Enter の
  GUI 実キー経路はセルフテスト 45b で回帰防止
- **IME 候補・未確定文字列の位置は shaping で出す**: `pane_cursor_origin` を
  col × セル幅の線形換算にすると全角行で打ち進めるほど右へずれる（描画は実フォント幅）。
  `cell_at` の逆写像（`ScreenLine::cell_cols` + `shape_line`）で求める
- **GPUI（taffy）の flex 子は overflow: visible だと自動最小サイズ = min-content**
  （2026-06-13 実機「下部ステータスバーが消える」の根因）: ルート flex 列の中段
  （flex_1）に min-height 制約が無く、サイドバー / パネルの内在コンテンツ高
  （ファイルツリー行数・tmux 一覧の量）がウィンドウ高を超えると中段が縮めず、
  ステータスバーが画面外へ押し出される（コンテンツ量依存なので再現が不定に見える）。
  対策: 中段に `min_h(0)` + タブバー / ステータスバー / パネルヘッダに `flex_none()`。
  **スクロールしない固定バーを flex 列に置くときは必ずこのペアを付ける**。
  Zed 本体が `min_h_0` を多用しているのも同じ理由

### ⚠️ UI スレッド同期処理のパフォーマンス教訓（2026-06-13 実機報告）

- **syntect ハイライトを UI スレッドで同期実行してはいけない**: release でも 200ms+
  （debug で 3s）。`preview::load_fast` で平文を即表示（0.8ms）し、ハイライトは
  `spawn_highlight` で background executor へ（色は後から付く 2 段階 UX）
- **render() 内で stat syscall を呼んではいけない**: `sync_filetree_roots` が各ペインの
  cwd に `is_dir()` を毎フレーム発行していた。OSC 7 由来の cwd は信頼して stat を
  省略し、削除された cwd は 2 秒ごとの `refresh` で回収する
- **定期ポーリング（2 秒タイマー）のファイル I/O は background へ**: `FileTree::refresh`
  の `read_dir_sorted` を main thread で回すと ~9ms/回。`refresh_targets` → background
  `scan_dirs` → main thread `apply_refresh` の 3 段階に分離
- **原則**: UI スレッド上で 1ms 以上かかるファイル I/O や CPU 計算を同期実行しない。
  やむを得ない場合は計測値をコメントに残し、非同期化の TODO を添える
- **dispatch でサブプロセスを同期起動してはいけない**（2026-07-13、#181 / #168）:
  `OrchestratorWorkerStatus` が `claude agents --json`（Node.js 起動 = 実測 500〜1100ms）を
  UI スレッドの dispatch 内で呼び、master のポーリング（1〜3 秒間隔）ごとに UI 全体が固まって
  「スクロールがカクつく」実機症状になった（perf.log に 2 時間で 2000 件超の dispatch 遅延）。
  対処 = IPC ループで UI 依存部分の収集だけを UI スレッドで行い、外部プロセスを叩く
  実行部を background executor へ（`dispatch::prepare_offload` / `OffloadJob`。GitLog /
  GitDiff も同機構）。同型の外部プロセス依存 dispatch を追加するときは OffloadJob へ
  ケースを足す（詳細は「メインスレッド非ブロック化とストール診断」節）

## コンセプト②の実現

技術選定（2026-06-12 ユーザー確認済み。候補比較は選定時のセッションログ）:

- **ファイルツリー**: OSC 7 で得た cwd をルートに表示。ツリー全体の更新は当面ポーリング。
  表示中プレビューの即時更新に限り `notify` の OS ネイティブイベントを使う（#233）
- **コードプレビュー**: シンタックスハイライトは **syntect**（bat / delta / gitui 採用の
  定番。言語セット同梱・導入容易、プレビュー中心 + 軽い編集の要件に十分）。
  ただし**ハイライタは小さな trait（`Highlighter`）で抽象化**し、編集機能が本格化したら
  tree-sitter（Zed と同じ構文木ベース・インクリメンタル）へ差し替え可能な構造にする。
  純 Rust 構成（`regex-fancy` 系 feature）にして oniguruma の C 依存は避ける（Windows CI）
- **Markdown**: **pulldown-cmark**（rustdoc / mdBook 採用のデファクト。イベントストリーム型で
  GPUI の独自描画に写しやすい）でパースし GPUI で描画
- **PDF**: 優先度 C。pdfium バインディング等、Phase 5 で要否ごと再判断
- **軽い編集**: `tako-core::TextBuffer` の UTF-8 `String` + 最小限の編集操作。プレビューの
  編集可能上限が 1MB / 5000 行で、ropey の利点が出る巨大文書は安全上編集不可にするため、
  新規依存 ropey は追加しない。LSP はやらない（Non-goal）
- **git graph**: **git CLI 子プロセス**（VS Code / lazygit と同方式。新規依存ゼロで
  tmux 取得層と同パターン、開発者環境に git は必ずある）で `git log --format` 等を
  パースして取得し GPUI で描画。gitoxide（API 発展途上）/ git2-rs（C 依存）は不採用。
  **2026-06-14 実装完了**: `tako-core::git` モジュール（`git_bin()` 解決 + log/branch/status/diff
  パーサ。ユニットテスト 5 本）。右パネルの git ビュー = ブランチ + 変更ファイル + コミット
  グラフ + diff 表示のアコーディオン。cwd 連動 2 秒ポーリング。dispatch `GitLog`/`GitDiff` +
  CLI `tako git log/diff` + MCP `tako_git_log`/`tako_git_diff`（計 25 ツール）

実装（2026-06-13。FR-3.1 改 / FR-3.2 / FR-3.3 完成）:

- **プレビューはペイン種別**: `tako-app::previews: HashMap<PaneId, PreviewState>`。
  載っているペインは render_pane が早期分岐してファイル内容を描く（PTY なし・
  attach_session を呼ばない）。読み込み・ハイライト・Markdown ブロック化は GPUI 非依存の
  `tako-app/src/preview.rs`（`Highlighter` trait + SyntectHighlighter。差し替え点）
- **操作は dispatch `OpenFile`** に一元化: UI クリック / `tako open` / MCP `tako_open_file`
  が同一経路。表示先解決（自身がプレビュー > 同タブ既存を再利用 > 分割新設）も dispatch 側。
  ControlHost に `preview_state` / `set_preview` / `preview_pane_of_tab` フックを追加
- **ファイルツリーは「タブ = ワークスペース」**: `sync_filetree_roots()` がアクティブタブ内
  全ペインの cwd を集めて `FileTree::set_roots()`（マルチルート。重複除去・既存ルートの
  展開状態維持）へ渡す。プレビューペイン（cwd なし）は自然にスキップされる
- **永続化**: layout.json の `PaneLayout.preview {path, mode}`（serde default で後方互換）。
  復元時は spawn せず `preview::load` で開き直す

実装（2026-07-12。FR-3.5 完成、#126）:

- **ドメインモデル**: `tako-core::text_edit::TextBuffer` に編集操作と保存競合検知を集約。
  UTF-8 バイト境界を不変条件とし、日本語の BS / Delete も 1 Unicode scalar 単位で扱う
- **UI**: `TakoApp::preview_edits` がペイン別バッファを保持し、`preview_render.rs` は描画した
  `StyledText` 自身の `TextLayout` をカーソル・選択へ再利用する（#145 で固定セル幅換算から変更）。
  逆写像は GPUI の raw glyph index と次の UTF-8 境界の実キャレット座標を比較して最近傍を選ぶ。
  ファイル / mode 差し替え時は旧座標キャッシュを即時無効化するため、Markdown の文字サイズ・
  日本語・タブ・スクロール・差し替え後も描画と逆写像が一致する。
  編集時は入力ごとの syntect 全再解析を避けて平文を即描画し、タイトルバーに編集切替・
  dirty・保存結果を表示。IME は既存
  `EntityInputHandler` を編集バッファへ振り分け、PDF / Markdown 読み取り選択は従来経路のまま
- **制御プレーン**: `ControlHost` の編集フックへ dispatch 3 操作を 1:1 で写し、CLI / MCP
  が同じ core バッファを操作する。未保存変更があるプレビューペインのファイル差し替えは拒否
- **PDF 選択（#145）**: PDFKit を明示リンクしてページ文字列・行矩形・文字矩形を抽出し、
  CoreGraphics のページ座標から表示画像座標へ変換する。PDFKit 未ロード時にテキストレイヤが
  無言で空になる回帰を、実文字矩形まで必須アサーションする macOS テストで防ぐ
- **PDF 選択の描画（#152）**: 画像と重ねる絶対配置 canvas は `.top_0().left_0()` を必須とする。
  省略すると GPUI の static position が直前の画像下端になり、誤った矩形同士の往復テストだけが
  通って実マウス座標と描画は外れる。文字矩形収集と選択描画を分離し、選択はペイン最終子の専用
  `paint_layer` で PDF の polychrome sprite より前面へ合成する。`visual-test` feature の Metal
  RGBA 読み戻しで PDF 選択と C++ / Python の読み取り・編集を対象矩形の実ピクセル差分まで検証する
- **PDF 選択の行間（#231）**: PDF の文字ヒットテストは行矩形内だけを有効とし、行間・
  ページ余白を文書末尾へクランプしない。選択開始時は操作を開始せず、ドラッグ中は直前の
  head を維持することで、行間から全テキスト選択へ化けることを防ぐ
- **PDF ラスタライズ品質（#231 / #234）**: 固定 2x ではなく、表示幅（64 logical px 単位）×
  `Window::scale_factor()`（1% 単位）× zoom（1% 単位）を `PdfRasterKey` とする。キー変更時は
  120ms debounce 後に background で全ページを再ラスタライズし、完了までは旧画像を表示する。
  `PreviewImageCache` も path だけでなく同じキーを比較し、古い PNG の `Arc<Image>` を誤再利用
  しない。実ピクセル幅はメモリ上限として 4096px にクランプする
- **PDF・画像ズーム（#234）**: 倍率・パン・PDF ページは GPUI 非依存の
  `tako-core::PreviewViewState` が検証・更新し、UI / dispatch / CLI / MCP は同じ操作へ載せる。
  UI はペインの `ScrollHandle` を再利用して 2 軸スクロールし、ピンチ中心または表示中心が
  拡大前後で動かないようパン量を補正する。PDF テキストレイヤは独自の倍率計算を持たず、
  スクロール適用後のページ画像 `Bounds` へ PDFKit の文字矩形を写像するため、ズーム・パン後も
  ヒットテストとハイライトが画像へ一致する。描画フレームでは既存 `Arc<Image>` の拡縮と
  座標変換だけを行い、再ラスタライズは上記 background 経路に限定する
- **プレビューライブリロード（#233）**: `notify` の OS ネイティブバックエンドで
  表示中ファイルの親ディレクトリだけを非再帰監視する。callback は channel 送信のみ、
  UI 側は 300ms デバウンスと対象スナップショット作成のみ、読み込み・syntect・
  pulldown-cmark・画像バイト・PDF ラスタライズは background executor で完了させる。
  世代照合に成功した結果だけを差し替え、`PreviewImageCache` をその時点で無効化する。
  重量読み込みは `(pane, path)` 単位の single-flight とし、実行中の変更は最新世代 1 件だけを
  完了後に再実行する。入力イベント数に比例して全ページラスタライズを並行させない（#258）。
  `preview_views` / `preview_scroll_handles` / mode は触らず #234 の状態を保持する。監視同期は
  open / close / 設定切替のみから呼び、render 毎フレームの処理は増やさない。
- **プレビュー目次（#232）**: GPUI / PDFKit 非依存の `tako-core::PreviewOutline` に表示ラベル・
  階層・Markdown ブロック番号または PDF ページ番号を保持する。Markdown は block パース直後、
  PDF は PDFKit `PDFDocument.outlineRoot` の background 走査で初回ロード結果へ同梱し、#233 の
  ライブリロードも同じ完成状態を世代単位で差し替える。render は `Arc<PreviewOutline>` の参照と
  パネル開閉中の行生成だけで、再パースや PDFKit 呼び出しをしない。ヘッダの目次と `n / total`
  ページ一覧は既存 `ScrollHandle::scroll_to_top_of_item` を使い、dispatch `PreviewOutline` + CLI
  `tako preview-outline` + MCP `tako_preview_outline`（一覧 / 項目ジャンプ）、既存 `PreviewView` +
  `tako preview --page` + `tako_preview_view`（ページ指定）を 1:1 で共有する。
- **syntect の行入力（#152）**: `SyntaxSet::load_defaults_newlines()` へ `str::lines()` の
  改行除去済み文字列を渡さない。`LinesWithEndings` で状態遷移に必要な改行を維持し、UI の行要素へ
  変換するときだけ末尾改行を除く。パス解決は読み取り / 編集で単一の `syntax_for_path` を使う

## Web ビューペイン（FR-3.8）→ ✅ wry ネイティブ統合で実装（2026-07-13、#155）

**GPUI には webview 要素が無い**ため、第一候補だった「ネイティブ webview の重ね合わせ」を
**wry 0.55**（Tauri の webview ライブラリ。Apache-2.0 OR MIT。macOS = WKWebView /
Windows = WebView2）で実装した。CDP ミラー方式 PoC（ヘッドレス Chrome +
スクショポーリング）は座標ずれ・入力中継の品質限界・Chrome 依存のため置き換え。

- **接続**: `gpui::Window` は `raw_window_handle::HasWindowHandle`（macOS では
  AppKitWindowHandle = GPUI の NSView）を実装しており、
  `wry::WebViewBuilder::build_as_child()` にそのまま渡せる。初回 render で
  `WindowHandleBox` に採取し、dispatch（IPC / MCP）からの生成でも使う
  （gpui-component の GPUI × wry 統合と同構成）
- **bounds 追従**: render_webview_pane が pane_text_areas と同じ絶対論理座標を
  `set_bounds`（Logical）へ渡す。差分呼び出しで AppKit 往復を抑制
- **入力**: OS がネイティブ webview へ直接配送（クリック・スクロール・キー・IME）。
  tako 側の中継は不要
- **タブ維持（dock）**: ページ = `WebViewEntry` を PaneId から独立管理。ペインを
  閉じても wry インスタンスが生き、SPA 状態・ログイン・スクロール位置が維持される。
  ステータスバー 🌐 ボタン → dock パネル（flex 列内 = webview と重ならない）から
  ワンクリック復帰。永続化は layout.json（PaneLayout.webview + LayoutFile.webview_dock）
- **可視性同期**: render 末尾で「今フレーム描画されなかった webview」（非アクティブ
  タブ・dock 退避）と D&D 中の全 webview を `set_visible(false)`。隠す際は
  `focus_parent()` でキー入力を GPUI へ返す
- **AI 操作**: dispatch `Request::Web`（action 式 9 操作）+ CLI `tako web` + MCP
  `tako_web`。JS 評価は 2 段階 API（eval → token → eval_result）— dispatch は
  UI スレッドで走り、wry のコールバックも UI スレッド配送のため同期待ちは
  デッドロックする（`webview.rs` の設計コメント参照）

**既知の制約（z オーダー）**: ネイティブビューは GPUI の GPU 合成レイヤの**上**に乗る。
GPUI のオーバーレイ（ピン留め窓 FR-2.16.15・ホバープレビュー・コンテキストメニュー・
注釈オーバーレイ FR-2.6）は webview ペインの上では隠れる。ドロワー・ステータスバー・
サイドバー・dock パネルは flex レイアウト内のため重ならない。D&D 中は全 webview を
隠してドロップターゲットを見せる。将来 FR-2.6 を webview 上でも使う場合は
オフスクリーン合成（CEF/Servo 級の重量）か webview 内 JS オーバーレイが要る。
スクリーンショット系機能（terminal_screen_lines 等）には webview の中身は映らない

## AI 誘導・注釈レイヤ（FR-2.6、後段フェーズ）

ペイン上のハイライト・指し示しは **GPUI の描画だけで完結する見込み**（deferred / overlay 描画、
ネイティブビュー不要）。対象指定は「ペイン ID + グリッド座標（行・列範囲）or 相対矩形」を
MCP / CLI から受け、UI 層がレイアウト矩形に変換して描画する。
入力はオーバーレイを素通しし、ユーザー操作・明示消去・タイムアウトで消す（FR-2.6.3）。
注意: Web ビューペイン上だけはネイティブビューが最前面になるため別方式が要る（上記リスク参照）。

## プラットフォーム抽象（platform/）

| 関心事 | macOS | Windows |
|---|---|---|
| PTY | openpty | ConPTY |
| IPC | Unix domain socket | Named pipe |
| プロセスツリー / listen ポート | libproc | Toolhelp32 / GetExtendedTcpTable |
| シェル統合 | zsh / bash / fish | PowerShell |

trait で抽象化し、core/ と control/ は platform/ の trait のみに依存する。

## 設定ファイル I/O の安全化（`tako-control::config_io`。#169）

2026-07-13、orchestrator の projects.yaml が並行 `projects add` で 58 件 → 1 件に
全消失した事故（Issue #169）の再発防止機構。根本原因は三段連鎖:

1. 旧 save が `std::fs::write`（truncate → write の 2 段階）で、並行プロセスに
   空 / 部分ファイルが見える窓があった
2. serde_yaml は空文字列・`projects:` だけの部分内容を「0 件」として**成功**パースする
   （`#[serde(default)]` + 空ドキュメント = null。エラーにならない）
3. read-modify-write に直列化がなく（GUI の MCP dispatch と CLI は別プロセス）、
   0 件を読んだ側の add が「その 1 件だけ」を書き戻して全件を消した

対策（`config_io.rs` に共通部品化。projects.yaml / profiles/*.yaml / config.yaml が使う）:

- **アトミック書き込み**: tmp ファイル（pid 付き）+ fsync + rename。並行 reader には
  旧内容か新内容しか見えない（空・書きかけの瞬間が存在しない）
- **プロセス間ロック**: `<path>.lock` の排他 flock（std `File::lock`、Rust 1.89+）で
  read-modify-write（`ProjectsConfig::mutate` / `Profile::mutate_named` /
  `setup::mutate_config`）を直列化。ロックファイルは削除しない（削除すると新旧
  2 つの inode を別プロセスが別々にロックでき排他が破れる）
- **fail-loud**: パースに失敗した既存ファイルへの mutate は f を呼ばず一切書き込まず
  Err。「default / 0 件に丸めて上書き」を全経路から排除
  （dispatch の profiles set にあった `unwrap_or_default()` 握りつぶしも修正）
- **世代バックアップ**: 内容が変わる書き込みの直前に `.bak.1`〜`.bak.3` を
  ローテーション（本体 → .bak.1 は copy。rename だと本体不在の瞬間が生まれるため）。
  内容不変の save は書き込みもバックアップ回転もしない
- 読み取り側はロック不要（rename により常に完全なスナップショットが見える）

## 多重インスタンスの資源保護（#177。復元強奪ガード + 縮退保存ガード）

2026-07-13、稼働中の本番 GUI から全ターミナルペインが消失した事故（Issue #177）の
再発防止機構。原因は多重起動ガード（#113）の構造的な穴:

1. ガードの判定材料は **discovery（control.json）だけ**だが、守るべき資源は
   **layout.json（HOME 固定）と tmux バックエンドセッション（TAKO_TMUX_SOCKET）**で、
   それぞれ独立した環境変数で差し替わる
2. `TAKO_DISCOVERY_DIR` だけ隔離した dev 検証インスタンスが「空の discovery」を見て
   プライマリ判定 → 本番 layout.json を復元 → `new-session -A -D` が本番 GUI の
   attach クライアント 13 本を無条件 detach（強奪）→ 本番側 PTY 一斉死亡（Exited）
3. タブごと消えた本番側の定期保存が縮退 layout（プレビューのみ）を上書きし、
   正常時の構成が失われた（実体セッションは tmux サーバー内で無傷）

対策（三層防御）:

- **復元強奪ガード**（`main.rs::foreign_client_guard`）: 復元より前に、layout.json
  記載の全セッションについて `tmux list-clients`（`client_pid`）を走査し、
  **生きた別 tako-app 配下のクライアント**が attach 中ならセカンダリモードへ降格する
  （復元しない・保存しない・固定ソケットを乗っ取らない）。判定材料を「守るべき
  資源そのもの」に置くため、隔離変数の組合せ・control.json の消失に依存しない。
  クライアント pid → 所有プロセスは `ps -axo pid=,ppid=` の祖先辿り
  （`agents::process_parent_map` 再利用）。手動 `tmux attach`（tako-app 祖先なし）は
  対象外、正当な再起動では旧クライアントが死んでいる（PTY 閉鎖 → SIGHUP）ため不発。
  `TAKO_SELF_TEST` / `TAKO_FORCE_PRIMARY` は従来どおり全ガードをバイパスする
- **縮退保存ガード**（`layout::save` → `backup_if_degraded`）: ペイン数が
  「直前 4 以上 → 半分未満」へ減る保存は、上書き前に直前の layout.json を
  `.bak.1`〜`.bak.3` へ退避（config_io の rotate を再利用）。Exited が 1 ペインずつ
  届く連鎖縮退（16→15→…→3）で健全世代が押し出されないよう、bak.1 が
  10 分以内の間は回転させない。復旧は `tako recover`（一覧 / `--apply <世代>`。
  稼働中 tako を検出したら拒否、`--force` で明示上書き）
- **一括隔離モード**（`TAKO_ISOLATED=1`）: discovery / persist / tmux socket を
  1 変数で全部隔離する。実験・検証起動の「片脚だけ隔離」ミス（今回の直接原因）を
  構造的に根絶する入口。個別変数が明示されていればそちらを尊重
- 補助: persist.log の全行に `[pid N]` を付与（複数インスタンスのログ混在を
  事後調査で切り分けられるように。今回の調査で pid 不明が解析を遅らせた）

## メインスレッド非ブロック化とストール診断（#168 / #115）

2026-07-13、「アプリ全体がちょくちょく止まる」「PDF・プロンプト入力がモサモサ」の
体感悪化を perf.log 実測で原因特定し、構造対策した。実測値と A/B 手順は Issue #168 参照。

原因（実測で確定した 3 犯）:

1. **dispatch の UI スレッド同期実行**: `OrchestratorWorkerStatus` が
   `claude agents --json`（ログインシェル + Node 起動 = 1 回 500ms〜1s）+ tmux + ps を
   UI スレッドで実行（3.3h で 4124 回・平均 687ms・UI 専有累計 47 分）。
   UI ストール（0.5s+）1021 件はすべてこれと共起
2. **PDF プレビューの毎フレーム再構築**: `gpui::Image::from_bytes` は id 生成で
   全バイトをハッシュするため、render 毎に呼ぶと全ページ PNG clone + フルハッシュで
   1 フレーム p50 96ms（71 ページ・通常 2ms）
3. **PDF / 動画ロードの UI スレッド同期実行**: 全ページラスタライズ + テキスト抽出で
   開く瞬間 1354ms ブロック

対策（三本柱。いずれも操作セマンティクスの一元化は維持）:

- **dispatch offload**（`dispatch::prepare_offload` / `OffloadJob`）: サブプロセス実行を
  伴う read-only リクエスト（OrchestratorWorkerStatus / GitLog / GitDiff）は、
  UI スレッドで文脈収集（workspace / ライブ画面の読み取り = µs オーダー）だけ行い、
  実行と応答送信を background executor へ逃がす。main.rs の IPC ループ 1 箇所の分岐で
  CLI / MCP 両経路に効く。dispatch 直呼び（UI 内部・テスト）は従来どおり同期。
  `TAKO_OFFLOAD=0` で旧経路（A/B 計測・切り分け用）。
  併せて `run_claude_agents_json` に TTL 2 秒キャッシュ + ロック直列化
  （多重 watch でも Node 起動が並走しない）
- **プレビュー描画キャッシュ**（`preview_render::PreviewImageCache`、#258）: PDF は
  表示ページの前後だけを `gpui::Image` へ遅延変換し、画像・動画サムネと合わせて
  `tako-core::ByteLru` へ推定デコード済み BGRA bytes を計上する。プロセス全体の既定予算は
  512MiB（設定 256〜8192MiB）。LRU / set_preview / close で外れた `Arc<Image>` は次の
  render 冒頭で `Image::remove_asset` と `App::drop_image` を呼び、GPUI の CPU asset cache と
  GPU sprite atlas の両方から除去する。動画の置換済み `RenderImage` も次フレーム冒頭で
  `drop_image` し、再生時間に比例して atlas texture を残さない。render 毎の処理は
  現在近傍 3 キーの HashMap 参照と
  LRU touch のみで、PNG clone / hash / eviction 走査は新規ページまたは世代変更時に限定する
- **重量プレビューの background ロード**（`pending_preview_loads` →
  `spawn_preview_load`）: PDF / 動画は Loading プレースホルダを即表示して
  `preview::load_fast` を background で実行、完了時に path 一致 + Loading 継続を
  確認して差し替え（後勝ち）。コード / MD / 画像は従来どおり同期（軽量）
- **ライブリロード経路のスパン（#233）**: `preview_watch_sync` / `preview_watch_event` /
  `preview_reload_apply` は、それぞれ監視集合の同期、イベント受理 + デバウンス予約、
  完成状態の差し替えだけを測る。ファイル I/O / パース / PDF 処理はこれらの UI
  スパン外の background にあり、アイドル時は event / apply 自体が 0 回となる

ストール診断（恒久・`tako-control::diag`）:

- `perf_span(tag)`: 重い区間の RAII 計測。32ms 超（`TAKO_PERF_VERBOSE=1` 時 16ms 超）を
  perf.log へ記録（1 秒 20 行のレート制限付き）。dispatch（種別名タグ）/ render /
  key_input / save_layout / link_scan / preview_load / ipc_turn / periodic_prep に設置。
  periodic_prep はステップ別サブスパン（`periodic_prep:tmux_ctx` / `:filetree_roots` /
  `:agent_metrics` / `:webview` / `:sleep_guard` / `:pane_log` / `:filetree_targets`）で
  攻撃者をステップ単位まで特定できる（#212）
- `spawn_stall_watchdog()`: 監視スレッド。区間が 2 秒を超えて継続中（ハング級）なら
  drop を待たず 1 回記録 =「止まった瞬間に何をしていたか」が残る
- `TAKO_PERF_VERBOSE=1` で 10 秒ごとにタグ別分布（count/p50/p95/p99/max）を出力、
  `TAKO_PERF_LOG=<path>` でログ先を差し替え（隔離実測が本番 perf.log と混ざらない）
- **UI スレッドでサブプロセスを実行しない**（#212 の教訓）: 2 秒毎の sleep guard AC 判定が
  `pmset -g batt` の同期実行で、アイドルでも 1 回 20〜30ms、cargo build 並走の CPU 飽和下では
  fork+exec が秒級に伸び「画面が重い・点滅・スクロールもっさり」の主犯になった。
  IOKit FFI（`IOPSGetTimeRemainingEstimate`）へ置換済み。定期パスに外部コマンドが必要なら
  background executor へ逃がすこと
- **プロセス走査の共有と変化検出**（#772 / #779）: `agents::ProcessSnapshot` が tmux pane PID と
  `ps` の親子関係を 1 回ずつ採取し、stale binary と sleep guard が同一 tick で共有する。
  sleep guard の 2 秒 tick は assertion の評価頻度であって、tmux / ps の採取頻度ではない。
  backend 集合・role・OSC 133 状態が変わったときと初回、取りこぼし回収の 60 秒ごとだけ再採取し、
  それ以外は前回の `busy_backend_sessions` を sleep guard / GUI モード / close 確認で共有する。
  `while-agents-running` へ切り替えた tick は、古いキャッシュで assertion を決めず即再採取する

## ビュー単位の描画キャッシュ（#782 / #786。2026-08-07）

tako は `TakoApp` 1 個をすべてのウィンドウのルートビューにしている（#339）。何もしないと
`cx.notify()` 1 回でアプリ全体（タブバー・サイドバー・右パネル・ステータスバー・表示中タブの
全ペイン）の element ツリーを作り直し、GPUI がそれを丸ごと taffy でレイアウトしてペイントする。
Zed では各エディタ・ターミナルが独立 entity なので、この浪費が構造的に存在しない。

段階的に埋めた（#782 が「見えていないものを描かない」、#786 が「見えているが変わっていない
ものを描き直さない」）:

- **可視性ゲート（#782）**: 画面のどこにも映っていないペイン（裏タブ・たまり場）の出力では
  再描画を要求しない。`pane_visibility` が「自分のペイン本体だけに映る（`OwnPane`）」
  「ドロワーのサムネイル・ホバー / ピン留めプレビューにも映る（`Elsewhere`）」
  「どこにも映らない（`Hidden`）」の 3 値を返す。判定不能な過渡状態は保守的に `Elsewhere`
- **ビュー単位キャッシュ（#786。`view_cache`）**: ペイン本体（`PaneBody`）とクローム 4 枚
  （`Chrome`: TabBar / Sidebar / Panel / StatusBar）を独立した子ビューにして
  `AnyView::cached(style)` で包む。汚れていないビューは GPUI が prepaint と paint を
  丸ごと再利用する。見た目のコードは `TakoApp` 側の既存 render メソッドのまま
  （子ビューは描画を委譲するだけ）
- **ヘッダの持ち上げ（#803）**: ペインのタイトルバー（`PaneHeader`）を本体の内側から
  **ルート側の兄弟**へ出し、独立したキャッシュ単位にした。`cached` は入れ子にできない
  （後述）ので、本体の中に置いたままでは「PTY 出力では変わらないのに毎フレーム作り直す」が
  構造的に避けられなかった。詳細は「ペインヘッダの持ち上げ」節

汚れ方の規約は 2 つだけ。**この順序が不変条件**（新しい状態変化を汚し忘れる事故が起きない）:

1. **PTY 出力**は `request_term_redraw` → `flush_term_redraw` 経由で、`OwnPane` のときだけ
   そのペインの `PaneBody` を notify する
2. **それ以外のすべての状態変化**は従来どおり `TakoApp` を notify する。子ビューは
   `cx.observe(TakoApp)` で自分も汚す

### ⚠️ 踏み抜きどころ

- **キャッシュしたビューは毎フレーム「アクセス」しないと二度と描き直されない**:
  `cx.notify()` がウィンドウの再描画に化けるのは、その entity が
  `App::tracked_entities`（= 直前の draw で accessed になった集合）に載っている
  あいだだけ。載っていないと observer を呼ぶだけで dirty を立てない。キャッシュが当たった
  フレームは GPUI が `element_state.accessed_entities` を積み直してくれるが、**その集合は
  初回 prepaint の差分**で作られるため、ビューを親の render の中で `cx.new` すると
  その id が差分から漏れて次フレームで tracked から外れる。`view_cache::cached_view` が
  毎フレーム `view.read(cx)` を通してこれを構造的に防いでいる
  （実測: プレビューペインが開いた直後の 1 フレームで固まり、目次ジャンプが効かなくなった）
- **ペインの配置はキャッシュビューのスタイルが持つ**: `AnyView::cached` の
  `request_layout` は**スタイルだけ**を見て大きさを決める（中身を見に行かない）ので、
  呼び出し側が絶対配置と大きさを確定させる。各 `render_*_pane` は矩形いっぱい
  （`relative` + `size_full`）に描く
- **状態を変えたら必ず notify する**: キャッシュが入る前は「毎フレーム全再構築」だったので、
  notify を忘れても次の draw で反映されていた。#786 の検証で visual-test に 1 箇所
  （編集モードの解除）その手抜きが残っていたのが発覚した
- **ホバー・アニメーションは自動で追従する**: hover / active / tooltip の状態変化は
  GPUI が `Window::refresh()` を呼び、`refresh` 中はキャッシュが無効になる。
  `with_animation` は `request_animation_frame` で**そのビュー自身**を notify するので、
  再描画のループが自走する
- **`AnyView::cached` は入れ子にできない**（#801 の実測 → #803 で回避）: GPUI は
  キャッシュビューを実際に描き直すあいだ `window.refreshing = true` を立て、再利用の条件に
  `!window.refreshing` が入っている。したがって**キャッシュビューの中のキャッシュビューは
  一度も当たらない**。ペインヘッダを `PaneBody` の内側でさらにキャッシュしても効かないので、
  #803 は**ヘッダをルート側の兄弟へ出した**（下の「ペインヘッダの持ち上げ」節）
- **`cached` は「汚れていても」得がある**（#801 の実測）: 中身は `layout_as_root` で
  確定サイズの別パスとして解かれるので、ルートの flexbox がその部分木を測り直さない。
  「どうせ描き直すのだからキャッシュを申し出ない」は逆効果で、汚れたペイン本体を
  素の箱で出しただけで **+0.86M instr/frame** 掛かった（119x21・空画面）
- 効果の回帰検出はセルフテスト項目 108（`pane_body_renders` / `chrome_renders` の増減）。
  `TAKO_786_NO_VIEW_CACHE=1` で同じバイナリの A/B が取れる

### 実測（隔離・色付き 110 桁 200 行/秒・A/B は同一バイナリ）

| 構成（表示中 2 ペイン・22x21） | before（キャッシュ無効） | after |
|---|---|---|
| 4 タブ + サイドバー + 右パネル | 25.30% CPU / 6.772M instr/frame | **18.04% / 5.016M** |
| 17 タブ + サイドバー（実フォルダ）+ 右パネル | 36.65% / 9.693M | **8.94% / 5.574M** |

クロームを 4 タブ → 17 タブへ増やしたときの 1 フレームあたり増分は
**2.92M → 0.56M（−81%）**。残る 5M 台はペイングリッドそのものの描画で、
専用 Element 化（#787。次節）の担当。

## 端末グリッドの専用 Element（#787。2026-08-11）

ペイン本体の端末グリッドは `tako-app/src/terminal_grid.rs` の `TerminalGrid`（GPUI の
`Element` を自前実装）が**1 要素で**描く。旧実装は 1 行 = 「行 div + スタイル区間ごとの
子 div」を毎フレーム taffy へ流していて、#782 の実測で **0.39M instructions / 行**
（137 桁で 2,800 命令/セル）掛かっていた。

描画は 2 段。**どちらもセルの原点を `col * cell_width` で直接決める**ので、
div の幅をデバイスピクセルへ丸めるぶんの累積が生まれない:

1. `plan_row`（純関数。GPUI の描画呼び出しを持たない = 単体テストできる）が `ScreenLine` を
   `RowPlan`（背景の帯 / シェイプ区間 / 下線 / 取り消し線）へ変換する。
   選択・ブロックカーソル・SGR 48 の背景色は `tako-core::screen` が既にランへ焼いてあり、
   ⌘ホバーのリンク装飾（#153）だけをここで上書きする
2. `Element::paint` が背景を `paint_quad`、グリフを `shape_line(..., force_width)` +
   `ShapedLine::paint`、下線・取り消し線を `paint_underline` / `paint_strikethrough` で置く

### この構造で直った既存バグ

- **#797（SGR 4 と ⌘ホバーのリンクの下線が 1 px も描かれない）**: GPUI の `paint_line` は
  下線を「ベースライン + descent×0.618」= 行ボックスの下端ちょうどへ置く。旧実装の行 div は
  高さ = セル高（17px）で `overflow_hidden`（#64 の折り返し対策で外せない）だったので、
  下線が丸ごと切り落とされていた。**element は下線を自分で引く**
  （`terminal_grid::underline_y` が「行の内側に収まる位置」を 1 か所で決める）
- **#798（全角が長く連なる行で描画位置が最大 1 セル左へ詰まる）**: 幅 `cell_width * cols` の
  div を 55 個積むと丸め不足が累積していた。element は列番号から座標を作るので 0 セル

### セル幅とグリフ幅の整合（#64 / #39 の対策を置き換えた仕組み）

`shape_line` の `force_width = cell_width` がグリフ位置をセル境界へスナップする
（Zed の端末 element と同じ）。これで **advance がセル幅と合わないグリフ（`⏺`・絵文字）の
後続文字が自動でグリッドへ戻る**ので、旧実装が #64 対策でやっていた
「不一致グリフを個別 div へ隔離して `overflow_hidden` で切る」が不要になった
（結果として絵文字は切られず実 advance のまま描かれる）。

`force_width` は「グリフ 1 個 = 1 セル」を仮定するので、**全角（2 セル）文字が占有する
2 セル目にはスペースを 1 個差し込む**（`shape_segments`）。これでグリフ数と列数が 1:1 に
戻り、全角が続く行でもスナップが効く。

### ⚠️ 踏み抜きどころ

- **行高はセル高を渡す**（`ShapedLine::paint(origin, cell.height, ...)`）。旧実装は
  `StyledText` 経由だったためベースラインが**環境の既定行高**（`13px × 1.618 ≒ 21px`）
  基準で決まり、セル高 17px の行 div の中で字が 2px 下へずれ、ディセンダ（g / j / p / q / y）が
  `overflow_hidden` で切れていた（実測: ディセンダのインクが行の最終デバイス行 33/34 まで
  詰まっていた → element では 31/34 で余白が残る）。**この 2px はユーザーに見える変化**
- **クリップは行単位ではなく element 単位**になる。行 div の `overflow_hidden` が無いので、
  背が高いフォールバックグリフが隣の行へわずかに滲む余地がある（Zed も同じ性質）
- **行の空白は落とす**（性能）。行頭 / 行末の空白は捨て、行中は 8 セル以上続いたところで
  シェイプ区間を切る。ただし**背景・下線・カーソルが乗るセルは別の層が描く**ので、
  「空白だから何も描かない」という判断をグリフ以外へ広げてはいけない
- 行レイアウトは GPUI 側で（テキスト・フォント・force_width をキーに）フレームをまたいで
  キャッシュされる。スクロールで同じ行が別の行位置へ移っただけならシェイプは走らない
- 行 div のスタック（`terminal_screen_lines`）は**残してある**。チャット入力欄のミラー
  （#719）・たまり場のサムネイル・タブツリーのホバープレビューは「行を他の要素の中へ
  埋め込む」用途なので、行が div のままの方が扱いやすい
- `TAKO_787_NO_GRID_ELEMENT=1` で旧経路へ戻せる（同一バイナリの A/B。効果測定と切り分け用）

### 回帰検出網

visual-test の `terminal-grid` 節（`TAKO_VISUAL_ONLY=terminal-grid` で単独実行）が
実ピクセルで固定している。#799 で先行整備した 6 検査に #787 で 4 つ足した:
下線が実ピクセルで出る / リンク下線が accent 色で出る / ディセンダまで 1 セルに収まる /
全角の長い連なりでも drift 0。**旧経路（`TAKO_787_NO_GRID_ELEMENT=1`）ではこの 4 つが
落ちる**ことを実測して検出力を裏取りしてある。

## 空白セルの近道（#801。2026-08-13）

グリッドの中身に関係なく、**画面を 1 枚描くたびに `cols * rows` セルぶんの変換**が
2 段走っていた。空画面（= 表示するものが何も無い状態）でも払うので、#782 → #786 →
#787 で削った後に残った固定費の最大項になっていた（119x21 で 1.76M instr/frame）。

近道は 2 か所。**どちらも「結果が既定値と同じになるセル / 行を、作らずに飛ばす」**だけで、
出力は 1 ビットも変わらない:

- `tako_core::screen::snapshot_opts`: 「素の空白セル」（半角スペース + 属性フラグ無し +
  前景・背景とも既定色）は `resolve_cell` の結果が `grid` の初期値と一致するので、
  解決も書き込みもしない。**既定色が OSC 4 / 11 で差し替えられている場合は近道を使わない**
  （判定はループの外で 1 回。`plain_blank_matches_default`）。
  1 セルも書かれなかった行は `compose_line` を 1 本だけ組んで複製する
- `tako_app::terminal_grid::plan_row`: 全部が空白で背景・下線・取り消し線がどのランにも
  無い行（`row_draws_nothing`）は `RowPlan::default()` を即返す。残る行も
  `Rgb -> Hsla` の変換を**ラン単位**に切り替えた（セルごとに引き直すと空画面でも
  毎フレーム 2,499 回走る）

### ⚠️ 踏み抜きどころ

- **「空白だから飛ばす」を属性へ広げない**: 下線・取り消し線・反転・DIM・明示背景色は
  空白セルでも見える。全角の右隣のスペーサー（`WIDE_CHAR_SPACER`）を素の空白として
  書き戻すと、`compose_line` が飛ばさなくなって列数が 1 ずれ `cell_cols` が壊れる。
  判定（`is_plain_blank`）は「フラグが 1 つも立っていない」ことを必要条件にしてある
- ⌘ホバーのリンク装飾は**空白セルにも**背景と下線を乗せるので、リンクのある行は
  `plan_row` の早期打ち切りの対象外
- `TAKO_801_NO_FAST_CELLS=1` で両方まとめて切れる（同一バイナリの A/B）

### 実測（隔離・grid-bench・300 フレーム・同一バイナリ A/B）

| 密度（119x27・ウェルカムバナー無し） | before | after |
|---|---|---|
| 空画面（= 固定費） | 3.584M instr/frame | **2.447M（−32%）** |
| 実務密度（918 セル） | 5.678M | **5.180M（−9%）** |
| 満杯（2,946 セル） | 8.605M | **8.411M（−2%）** |

残る固定費の内訳（空画面・段階的無効化ゲートの差分。**目標の 1M には未達**）:
ウェルカムバナー #549 1.17M（**初回起動時のみ表示**）/ ペインヘッダ 0.62〜0.68M（当時は
上記の「`cached` は入れ子にできない」で塞がっていた → #803 で持ち上げ）/
スナップショット残り 0.65M / クローム 4 枚の cached 再利用 0.46M / ルートの箱 0.41M /
gpui のフレーム下限 0.16M。
**ルートが空 div を返すだけのフレームが 0.159M** なので、gpui 側の下限は十分小さく、
残りはすべて tako の要素ツリーの大きさに比例する。

## worker への指示送達の 2 層化（#790。2026-08-14）

master → worker の指示は長らくキー操作（貼り付け + 分離 Enter + 空検証）1 本だった。
claude v2.1.224+ の Cross-Session Messaging を第 1 層に足し、**使えなければ従来経路へ
落ちる**構成にした。実装は `tako-control::peer_messaging`（発見・可用性判定・送信・受信確認）
と `tako-control::delivery`（経路選択とログ）。要件は FR-2.2.2 追補 2。

### 伝送と発見（2026-08-14 に v2.1.232 で実測）

```
<config dir>/sessions/<pid>.json        レジストリ（messagingSocketPath / peerProtocol /
                                        kind / status / version / tmux / cwd …）
<config dir>/sessions/<pid>.<hash>.key  {"peerToken": …, "procStart": …}（0600）
/tmp/cc-socks/<pid>.sock                受信箱（0600）
```

socket へ接続して改行区切り JSON を 2 行書く（`{"type":"auth","token":…}` →
`{"type":"user","message":{"role":"user","content":…}}`）。受信側は `LOCAL_PEERCRED` で
送信元 pid を検証し、transcript に `origin: {kind:"peer", verifiedPeerPid}` を残す。

### なぜ「worker 宛だけ」なのか

本文には tako から抑制できない定型の前置きが付く（「別の claude セッションから届いた」
「peer は権限昇格を与えない」「**保留中プロンプトの承認として扱うな**」）。master → worker は
その関係そのものなので正確だが、人が `tako send` で「はい、進めて」と送る用法では意味が
変わる。だから宛先の role で分ける（`agent_managed_role`）。チャット入力欄は PTY の
ミラー（#718 / #719）でこの経路を通らないので、GUI で人が打つ操作は影響を受けない。

### 実測（Issue #790 のコメントに全量）

| 状況 | 結果 |
|---|---|
| idle | ターンとして処理（transcript `origin.kind=peer`） |
| busy（生成中） | キュー投函 → ターン終了後に処理。取りこぼしゼロ |
| permission ダイアログ中 | **ダイアログ無傷**でキューへ入り、進行中ターンの `attachment/queued_command` として取り込まれる |
| 長文 | 28,101 文字 / 43,449 バイトを 1 回でバイト等価に送達（先頭〜末尾まで欠落なし） |
| 受信箱の bind | claude 起動から 1.1 秒（入力欄表示とほぼ同時） |

### ⚠️ 踏み抜きどころ

- **送り切った後にフォールバックしない**。socket へ書き切った時点で受信側のキューに
  入るので、そこから従来経路へ落ちると同じ指示が 2 回届く。落ちてよいのは可用性判定と
  接続失敗（1 バイトも読まれていないと言える段階）だけ
- **可用性はサーバー側 gate（GrowthBook）依存で env では強制できない**。off のセッションは
  受信箱を開かない = レジストリに `messagingSocketPath` が出ない。`claude --help` に
  該当フラグは無く、`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`（upstream #35240）は
  v2.1.232 のバイナリに文字列として存在しない
- **時刻文字列で受信を判定しない**。`sessions::now_iso()` は秒精度（`…:21Z`）で transcript は
  ミリ秒（`…:21.218Z`）なので、辞書順では同じ秒の痕跡が送信時刻より前に並ぶ。
  送信直前のファイル長を控えて追記分だけ読む（`TranscriptCursor`）
- **受信の姿は 2 形態**。`user/isMeta` だけを見るとダイアログ中・生成中の送達
  （`attachment/queued_command`）を「未達」と誤判定する
- **UI スレッドで待たない**。socket 接続 + 受信確認は待ちを含むので background スレッドへ
  出し、`PromptFlow` は結果のスロットを覗くだけにする（#212 / #772 と同じ規約）
- 「どれが claude の pid か」をコマンドパスで判定しない。**レジストリにエントリがあるか**で
  決める（ペイン自身が claude の場合と、シェルの子である場合の両方を 1 つの規則で扱える）
## ペインヘッダの持ち上げ（#803。2026-08-15）

ペインのタイトルバーに出るもの（タイトル・role・状態ドット・番号・cwd・workers）は
**PTY の出力では変わらない**のに、#801 までは出力のたびに作り直していた。ペイン本体
（`PaneBody`）の内側にあるため、そこでキャッシュしても入れ子になって一度も当たらない
（上記の制約）。#803 はヘッダをペイン本体の**兄弟**（どちらもルートの子）へ出し、
`view_cache::PaneHeader` として独立したキャッシュ単位にした。

- ルートは各ペインについて「本体（矩形いっぱい）」と「ヘッダ（矩形の上端）」を並べる。
  ヘッダの外側の div は**ペイン枠と同じ箱**（同じ矩形・同じ枠幅・同じ角丸・`overflow_hidden`）を
  taffy に組ませるためのもので、背景も影も持たない。位置を px で手計算しない
  （「枠の内側の上端」という関係をレイアウトエンジンに解かせる）
- **枠線だけは外側の div にも描かせる**（`border_color` を同じ規則で入れる）。
  GPUI の `Style::paint` は「影 → 背景 → 子 → 枠線」の順なので、持ち上げる前は
  ペイン枠の丸め角が**ヘッダの上**に来ていた。兄弟にすると本体の枠線より後に塗られ、
  上 2 つの丸め角がヘッダの四角い背景で潰れる（実測: フォーカス枠の accent が
  角で 104px 消えた）。子（ヘッダ）の後に同じ矩形・同じ色の枠線を塗り直すことで
  重なり順を戻す。丸め角の値は `PANE_CORNER_RADIUS` の 1 か所から取る
- ペイン本体は同じ高さ（`PANE_TITLE_BAR`）の空きスペーサーでヘッダの場所を空ける。
  `pane_text_areas` の会計（`stacked_top`）は持ち上げの前後で変わらない
- **ヘッダを出すかどうかは「本体が実際に場所を空けたか」で決める**（`lifted_header_panes`）。
  live な判定（`pane_display_for`）で決めると、本体がキャッシュ再利用のまま表示種別だけが
  変わったフレームでヘッダが二重に出る / 消える。Web ビュー・プレビュー・スターター・
  チャット・準備中の各表示は**自前のヘッダ**を持つので対象外
- **ヘッダの時計（`running · 4m12s`）だけは時間で変わる**ので、`tick_pane_header_clocks` が
  1 秒に 1 回だけ Running のペインのヘッダを汚す（出力が止まっている running ペインの
  時計は、持ち上げ前より新鮮になる）
- 回帰検出はセルフテスト項目 110（`pane_header_renders` の増減 + 実描画矩形とテキスト領域の
  正の関係）と番犬テスト（`self.render_pane_header(` が main.rs から消えていること）。
  `TAKO_803_NO_HEADER_CACHE=1` でヘッダのキャッシュだけを切れる

### 実測（隔離・grid-bench・300 フレーム・`main`(cb06672) と改修版の**交互 3 反復の中央値**）

| 密度（119x27・ウェルカムバナー無し） | main | #803 | 差 |
|---|---|---|---|
| 空画面（= 固定費） | 2.192M instr/frame | **1.737M** | **−0.455M（−21%）** |
| 実務密度（915 セル） | 5.158M | **4.698M** | −0.460M（−9%） |
| 満杯（2,943 セル） | 8.435M | **7.977M** | −0.458M（−5%） |

上限（= ヘッダを丸ごと描かないゲートを当てた `main`）は空画面 **1.517M** なので、
**ヘッダの総コストは 0.678M**、そのうち **0.455M（67%）を回収**した。残る 0.22M は
`AnyView::cached` の再利用そのもの（`reuse_prepaint` / `reuse_paint` / `Scene::replay`。
#801 でクローム 1 枚あたり約 0.115M と実測済み）と外側の div で、tako 側から削る余地は薄い。
**#801 が見積もった 0.81M は 119x21 + バナーありの構成での値**で、本節の構成
（119x27・バナー無し）でのヘッダ実測は 0.678M。

grid-bench は `renders=(body +300 header +0 chrome +0)` を出す（300 フレームの出力で
ヘッダの再描画が 0 回 = キャッシュが 100% 当たっている）。「キャッシュが当たっていない」と
「当たっているが再利用が重い」を取り違えないための計測点。

### 見た目の確認（実フレームの全ピクセル比較）

visual-test の `TAKO_VISUAL_DUMP_DIR` で `main` と改修版の実フレーム（2200x1416）を
撮って全画素を突き合わせた。**残る差は 32/3,115,200 画素（0.001%）**で、
上 2 つの丸め角の**枠線のアンチエイリアス縁だけ**（最大 45/255）。
持ち上げ前は枠線が 1 回、持ち上げ後は本体と外側 div の 2 回塗られるため、
半透明の縁が二重合成されてわずかに濃くなる。**色・太さ・位置は同じ**で、
枠線を塗り直さなかったときの 104 画素（accent が角で消える = 目に見える欠け）とは別物。
visual-test の 98 個のピクセル計測値は `main` と一致する（差は md の読み込み ms のみ）。

## 構文セットの寿命（#815。2026-08-15）

コードプレビューを 1 枚開くと tako 自身のヒープが 100 MB 級で増え、**閉じても戻らない**
（#814 の実測監査）。原因は `preview::highlighter()` が `OnceLock` のプロセス常駐だったこと
**だけではない**。

### 実費は「セット」ではなく「使った言語」にある（計測で確定）

カウンタ付きグローバルアロケータで live バイトを直接数えた結果（同一構成の使い捨てハーネス）:

| 段階 | live | 所要 |
|---|---|---|
| `two_face::syntax::extra_newlines()`（213 構文） | **1.04 MB** | 1.2 ms |
| `ThemeSet::load_defaults()`（7 テーマ。1 枚だけ残す） | 0.11 MB | 0.3 ms |
| ↑ のセットで **Rust** をハイライト | **+5.1 MB** | 11 ms |
| 同 **bash** | +10.9 MB | 14 ms |
| 同 **Markdown** | +10.9 MB | 27 ms |
| 同 **TypeScript** | **+32.0 MB** | 68 ms |
| 18 言語を順に通した累計 | **149 MB** | — |

syntect 5 の `SyntaxSet` は**コンテキストと正規表現を初回使用時に遅延展開**し、それを
セットの内側へ溜める（`SyntaxSet` 自体は 1 MB の器にすぎない）。**言語単位で捨てる API は
無い**ので、捨てられる単位はセット全体だけ。2 枚目の別言語で増える（#814 の +17.6 MB）のも、
ファイルの大小より言語の重さで決まるのもこれが理由。

### 採らなかった案（Issue の推奨案は計測で棄却）

- **段階ロード**（既定は syntect 同梱の軽いセット、無い拡張子だけ two-face へ昇格）:
  器の差は **1.04 MB → 0.40 MB の 0.64 MB だけ**で、実費（言語ごとの展開）は変わらない。
  一方で既定セットに無い拡張子が **363 件**（`.ts` `.toml` `.swift` `.zig` `.vue` `.proto` …）
  あり、#320 の対応言語を縮退させる回帰リスクだけが残る
- **`ThemeSet::load_defaults()` の廃止**: 0.11 MB。既に 1 枚を残して捨てているので効果なし

### 採った案: 借用チケット（`SyntaxLease`）+ 無使用の解放

- `highlighter()` は `&'static dyn Highlighter` ではなく **`SyntaxLease`（`Arc` の借用）**を返す。
  **チケットが生きている間は解放されない**ので、background のハイライト中に足元が消えることが
  型として起こり得ない（実解放は最後のチケットが落ちた時に `Arc` の規則で起こる）
- `SyntaxCache` が「猶予中の保持」を 1 本だけ持ち、2 秒 tick
  （`periodic_prep:syntax_release`）が `release_idle_syntax` で手放す。
  テキストのプレビューが 1 枚も無ければ猶予を待たない。開いている間は
  `SYNTAX_IDLE_GRACE`（30 秒）まで保持する（編集の連続打鍵・ライブリロードの連投で
  ロードし直さないため）
- **開いたままでも猶予経過で手放してよい**のがこの設計の要。表示中の色は
  `PreviewContent::Code` が所有済みで、構文セットは*再*ハイライトのときしか要らない。
  再取得は実測 0.6〜1.2 ms + その言語の初回展開（3629 行の Rust を再ハイライトする全体で
  167 ms → 211 ms）で、初回ロードもライブリロードも background 経路
- 寿命ロジックは `SyntaxCache` に閉じており、グローバルはその薄い包み。
  単体テストはローカルの `SyntaxCache` に対して行うので並列テストで揺れない
- **払うことになるコスト（承知の上）**: チャットビューの md（`chat_md_blocks`）は
  **コードブロックを含む発話が来たときだけ** ハイライタを借りる（`highlighter()` は
  `Event::End(TagEnd::CodeBlock)` の中でしか呼ばれない）。静かな時間が猶予を超えた直後に
  そういう発話が届くと、その 1 フレームだけ冷えた展開（md +27 ms・TS なら +68 ms）を
  UI スレッドで払う。生成中は発話が更新されるたびに温まり直すので、払うのは
  「静かな時間のあとの 1 回」だけ。100 MB 級の常駐と引き換えに受け入れている
- 回帰検出はセルフテスト項目 112（実 dispatch でプレビューを開く → 載る → 猶予内は
  手放さない → 閉じたら手放す → 開き直すと色が戻る）と、拡張子の全数解決テスト
  （構文 200 未満 / 拡張子 550 未満で落ちる = セットを軽い方へ差し替えたら気づく）。
  `TAKO_815_NO_SYNTAX_RELEASE=1` で旧挙動（常駐）へ戻して同一バイナリ A/B ができる

## コードプレビューの仮想化（#821。2026-08-15）

コードプレビューは**ファイル全行ぶんの element を毎フレーム**作っていた。3,884 行なら
1 行あたり div ×3 + `StyledText` ×2（行番号 + 本文）+ canvas ×1 で、1 フレーム約 2 万個。
これが「閉じても戻らないヒープ」の正体だった。

### 残留の機序（allocation プロファイルで確定）

シンボル付き release + `MallocStackLogging=1` の `heap <pid>` で、閉じたあとの live を
確保元ごとに見ると先頭がこうなる（3,884 行 .rs を 1 回開閉した直後）:

| 確保元 | live | ブロック |
|---|---|---|
| `RawVecInner::finish_grow`（巨大 Vec 3 本） | 42.9 MB | 3 |
| `WindowTextSystem::shape_text` の `Vec<DecorationRun>`（3072 B 固定） | 24.2 MB | **7,888** |
| `gpui::arena::Chunk::new`（element アリーナ） | 21.3 MB | 20 |
| `MacTextSystem::layout_line` | 4.0 MB | 14,592 |
| `TaffyLayoutEngine::request_measured_layout`（測定クロージャの Box） | 2.5 MB | **7,883** |

**7,883 ≒ 2 × 行数 = 1 フレームぶんの測定レイアウトノード**。
gpui の `TextLayout::layout` は `request_measured_layout` へ渡すクロージャに
`TextLayout`（整形済みの `WrappedLine` ごと）をキャプチャさせ、それが taffy の
`node_context_data` に入る。そして **taffy 0.10.1 の `TaffyTree::clear()` は
`nodes` / `children` / `parents` しか消さず `node_context_data` を消さない**
（`SecondaryMap` の残骸は同じ slot index が再利用されるまで生き続ける）。
gpui は毎フレーム `clear()` を呼ぶので、**「今までで一番大きかったフレーム」の
測定ノードとそこから辿れる整形済みテキストが永久に居座る**。
残る 65 MB 級はアリーナのチャンクとフレーム用 Vec の**高水位**で、これも
「1 フレームで 2 万個」が作った山がそのまま残ったもの。

close 時の解放をいくら足しても直らない（実測: 閉じたあと 300 フレーム描いても
残留は 1 バイトも減らない）。**ピークを作らないことだけが効く**。

### 採った形

- 本文は `gpui::list`（可変高さ + 遅延計測）で**可視行だけ** element を作る。
  折り返しがあるので行高は一定でなく、高さの実測は list に任せる（`uniform_list` は使えない）
- 1 行の作り方は `render_preview_code_line` の 1 実装。旧挙動（全行）も同じ関数を
  通すので、`TAKO_821_NO_VIRTUAL_LIST=1` の同一バイナリ A/B で**絵が変わらない**
- 索引は常に文書の行番号で、`preview_text_layouts` は可視行だけ `Some`。
  読む側（`preview_text_layout_hit_test` / `md_link_at_position`）は元から `None` を
  読み飛ばす形だったのでそのまま動く。`preview_line_texts` は**全行ぶん**を持つ
  （⌘A・コピー・ヒットテストの正）

### ⚠️ 踏み抜きどころ

- **list の item は伸ばしてくれる親を持たない**（`layout_as_root` で単独に解かれる）。
  幅を指定しないと行が content 幅で解かれ、**長い行の折り返しが消える**
  （実測: 410px の行が 3272px の 1 行になった）。行に `w_full` が要る
- **未 prepaint の `TextLayout` を外へ出すとプロセスごと落ちる**。gpui の
  `bounds()` / `index_for_position()` / `position_for_index()` は `bounds` を
  unwrap する。list は高さの見積もりで item を `layout_as_root` するだけのことが
  あるので、レイアウトの控えは**キャレット canvas の paint 時**に入れる（要素は増えない）
- **余白はリスト側に置く**。div スクロールは overflow を padding box でクリップするので
  内容が余白の上まで描かれるが、list は content box の内側にしか描けない。
  コンテナに padding を残すと上下の端で数 px ぶん絵が変わる（実測で検出した）
- ドラッグ選択のオートスクロール（#309）はビューポート矩形とスクロールの当て先が
  器ごとに違う。`preview_viewport_bounds` で吸収してある

### 実測（隔離・同一バイナリ A/B。`TAKO_821_NO_VIRTUAL_LIST=1` が旧挙動）

3,884 行の `.rs` を開閉 3 往復（live ヒープ = 全 malloc ゾーンの `size_in_use`）:

| 段階 | before | after |
|---|---|---|
| 起動直後 | 11.57 MB | 11.58 MB |
| 開いた（1 回目） | 124.03（+112.5） | **14.85（+3.3）** |
| **閉じた（1 回目）** | 121.71（**残留 110.1**） | **13.80（残留 2.2）** |
| 閉じた（2 回目） | 158.71（残留 147.1） | 14.02（残留 2.5） |
| 閉じた（3 回目） | 158.89（残留 147.3） | 14.18（残留 2.6） |
| 整形した行数 | 3,884 | **11**（可視ぶん） |
| 定常フレーム | 0.94〜1.00 ms | **0.12〜0.13 ms** |

1 万行（表示上限で 5,000 行）では footprint が **210 MB → 46 MB**。

### 見た目が変わっていないことの確認

`TAKO_VISUAL_ONLY=preview-code` の節を旧挙動と並べると、**本文領域の実ピクセル差は 0**
（行の実描画矩形・折り返し行の高さ 189px・スクロール量 600px・ドラッグ選択の
掴んだ行数 49・コピー 1,449 文字まで一致）。visual-test 全 98 チェックポイントも
`cpp` / `python` / `subline` / `content-geom` / `indent-guide` / `md` が完全一致する。

### CLI / MCP の close も同じ後始末を通る（#821 で見つけた別バグ）

GUI の close（`remove_pane_with`）と CLI / MCP の close（`detach_session`）が
**それぞれ独自にフィールドを列挙**していたため、後から足したものが片方だけに入り、
CLI / MCP で閉じたコードプレビューは行テキストと行レイアウトを落とさなかった
（3,884 行で 1 回の開閉あたり約 0.8 MB がプロセス終了まで残る）。
一式は `TakoApp::drop_preview_pane_state` に集約し、番犬テスト
`preview_cleanup_watchdog` が「独自列挙が復活していないこと」を CI で拘束する。

## セキュリティ方針

- IPC / MCP は localhost のみ + セッション毎のランダムトークン必須（FR-2.3.4）
- `tako send` / `tako_send_input` は任意コマンド実行と等価な力を持つため、
  「トークンを持つ = アプリ内で起動されたプロセスのみ」が防御線
- リモート接続は Tailscale Serve 経由のみ（tailnet 内限定・WireGuard E2E 暗号化。
  daemon は 127.0.0.1 bind。認証は二層: ① Tailscale identity + ② 機器ペアリング。
  公開インターネットへの露出なし。脅威モデルは `.agent/threat-model-remote.md`）
