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
4. **ライセンス互換性**: GPUI は Apache-2.0、alacritty_terminal は Apache-2.0 で、
   tako の Apache-2.0 と互換。cmux（GPL-3.0）のコードは絶対に読まない（`concept.md` 参照）。
   **zed リポ内の gpui 以外のクレート（terminal 等）は GPL 系のため同様にコードを読まない**
   （gpui / gpui_platform 等 Apache-2.0 のものだけ参照可）。

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
  設計原則 5（UI でできることはすべて AI からもできる）のセマンティクスを一箇所に保つ
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

#### 公開ツール（実装済み 12 個。FR-2.5 と 1:1）

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

## コンセプト②の実現

技術選定（2026-06-12 ユーザー確認済み。候補比較は選定時のセッションログ）:

- **ファイルツリー**: OSC 7 で得た cwd をルートに表示。更新は当面ポーリング
  （notify クレートの導入は必要性が出たら再判断）
- **コードプレビュー**: シンタックスハイライトは **syntect**（bat / delta / gitui 採用の
  定番。言語セット同梱・導入容易、プレビュー中心 + 軽い編集の要件に十分）。
  ただし**ハイライタは小さな trait（`Highlighter`）で抽象化**し、編集機能が本格化したら
  tree-sitter（Zed と同じ構文木ベース・インクリメンタル）へ差し替え可能な構造にする。
  純 Rust 構成（`regex-fancy` 系 feature）にして oniguruma の C 依存は避ける（Windows CI）
- **Markdown**: **pulldown-cmark**（rustdoc / mdBook 採用のデファクト。イベントストリーム型で
  GPUI の独自描画に写しやすい）でパースし GPUI で描画
- **PDF**: 優先度 C。pdfium バインディング等、Phase 5 で要否ごと再判断
- **軽い編集**: ロープ構造（ropey）+ 最小限の編集操作。LSP はやらない（Non-goal）
- **git graph**: **git CLI 子プロセス**（VS Code / lazygit と同方式。新規依存ゼロで
  tmux 取得層と同パターン、開発者環境に git は必ずある）で `git log --format` 等を
  パースして取得し GPUI で描画。gitoxide（API 発展途上）/ git2-rs（C 依存）は不採用

## Web ビューペイン（FR-3.8、後段フェーズ）

**GPUI には webview 要素が無い**ため、実現には GPUI の外の仕組みが要る。
候補と技術リスク（採否は後段フェーズの検証スパイクで判断、`roadmap.md`）:

1. **ネイティブ webview の重ね合わせ（第一候補）**:
   macOS は WKWebView を NSView として GPUI ウィンドウへ addSubview、
   Windows は WebView2 を子 HWND として配置し、ペイン矩形に位置・サイズを追従させる。
   ランタイム同梱不要で軽い（NFR-3/7 と整合）。
   リスク: GPUI の GPU 合成レイヤとネイティブビューの z オーダー協調
   （GPUI 描画はネイティブビューの**下**にしか出せない＝注釈オーバーレイ FR-2.6 と干渉）、
   タブ切替・分割変更時の表示/非表示とクリッピング、フォーカス・IME・キーボードショートカットの
   ルーティング分断、スクリーンショット系機能に映らない
2. **オフスクリーンレンダリング**: CEF / Servo 系でテクスチャ化して GPUI 内で描画。
   z オーダー問題は消えるが、依存が巨大でメモリ・配布サイズが NFR-3/7 に反する。保留
3. **外部ブラウザ起動（フォールバック）**: ペイン統合を諦め `open <url>` する。
   Phase 5 以前でも `tako_open_url` の暫定実装として先行提供できる

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

## セキュリティ方針

- IPC / MCP は localhost のみ + セッション毎のランダムトークン必須（FR-2.3.4）
- `tako send` / `tako_send_input` は任意コマンド実行と等価な力を持つため、
  「トークンを持つ = アプリ内で起動されたプロセスのみ」が防御線
- リモート接続機能は持たない（Non-goal: クラウド機能なし）
